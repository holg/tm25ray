//! IES TM-25 ray files (`.TM25RAY`): zero-copy and streaming readers, a
//! writer, flux-preserving subsampling and a far-field C/γ converter.
//!
//! TM-25 describes a light source as individual rays (start point in mm,
//! unit direction, flux) rather than an angular intensity table, so it is
//! the near-field counterpart of EULUMDAT/IES LM-63.
//!
//! # Layout (TM-25-13, little-endian)
//!
//! Verified against real vendor files and the field order of the reference
//! implementation; see `docs/format.md` in the repository for the full
//! table and the remaining `VERIFY` items.
//!
//! | Offset | Content |
//! |---|---|
//! | 0 | `TM25` magic, i32 version 2013, i32 creation method |
//! | 12 | f32 luminous flux lm, f32 radiant flux W, u64 ray count |
//! | 28 | 28-byte ASCII date/time, i32 start-position flag |
//! | 60 | i32 spectral id (0 none, 1 single λ, 2 per-ray λ, 3 shared table, 4 per-ray table index) |
//! | 64 | f32 single λ (NaN if unused), f32 λ min, f32 λ max |
//! | 76 | i32 spectral table count, i32 additional item count, i32 additional text size, reserved to 256 |
//! | 256 | 8 × i32 known-data flags: position, direction, radiant flux, wavelength, luminous flux, Stokes, tristimulus, spectrum index |
//! | 288 | 9 × 1000 UTF-32LE description strings |
//! | 36 288 | spectral tables (i32 N, N × (f32 λ, f32 value)), column-name block, additional text |
//! | then | ray records: `x y z kx ky kz` + flagged columns, all f32 |
//!
//! # Coordinates
//!
//! TM-25 positions are mm with +z the emission axis. [`direction_to_c_gamma`]
//! gives EULUMDAT angles (γ = 0 on +z, C from +x towards +y) and [`to_bevy`]
//! maps to the y-up convention used by the eulumdat-rs Bevy viewer
//! (`x, −z, y`) so a ray-derived solid overlays an LDT solid.
//!
//! # WASM
//!
//! The core is `std`-only and never touches the clock. `mmap` is an opt-in
//! feature for native builds. Browser callers feed `Blob.slice` chunks to
//! [`Tm25Reader`] through any `Read` adapter.

pub mod error;
pub mod farfield;
pub mod header;
#[cfg(feature = "mmap")]
pub mod mmap;
pub mod ray;
pub mod reader;
pub mod sample;
pub mod writer;

pub use error::{Result, Tm25Error};
pub use farfield::{FarField, FarFieldBuilder, IntensityUnit};
pub use header::{
    columns_from_flags, Column, FluxKind, Header, KnownDataFlags, ParseOptions, SpectralId,
    SpectralTable, TextFields, FIXED_HEADER_SIZE, MAGIC, VERSION_2013,
};
#[cfg(feature = "mmap")]
pub use mmap::Tm25Mmap;
pub use ray::{direction_to_c_gamma, from_bevy, to_bevy, Ray, RayIter, RayLayout, RayView};
pub use reader::{RayStream, Tm25File, Tm25Reader};
pub use sample::{subsample, Reservoir, Rng64};
pub use writer::{encode_header, to_bytes, write_tm25};

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    /// Synthetic Lambertian emitter on a 2 mm square, `n` rays of `flux_each`.
    fn lambertian_rays(n: usize, flux_each: f32, seed: u64) -> Vec<Ray> {
        let mut rng = Rng64::new(seed);
        (0..n)
            .map(|_| {
                let u = rng.next_f64();
                let v = rng.next_f64();
                // Cosine-weighted hemisphere: γ = asin(√u), uniform azimuth.
                let sin_g = u.sqrt();
                let cos_g = (1.0 - u).sqrt();
                let phi = 2.0 * std::f64::consts::PI * v;
                let x = (rng.next_f64() * 2.0 - 1.0) as f32;
                let y = (rng.next_f64() * 2.0 - 1.0) as f32;
                Ray::new(
                    [x, y, -0.5],
                    [
                        (sin_g * phi.cos()) as f32,
                        (sin_g * phi.sin()) as f32,
                        cos_g as f32,
                    ],
                )
                .with_radiant_flux(flux_each)
            })
            .collect()
    }

    fn header_uv() -> Header {
        let mut h = Header::new(KnownDataFlags::radiometric());
        h.creation_method = -1;
        h.radiant_flux_w = 0.051;
        h.date_time = "2026-05-12 13:57:44".into();
        h.spectral_id = SpectralId::SharedTable;
        h.min_wavelength_nm = Some(220.0);
        h.max_wavelength_nm = Some(320.0);
        h.spectra = vec![SpectralTable {
            wavelengths_nm: (0..51).map(|i| 220.0 + 2.0 * i as f32).collect(),
            values: (0..51).map(|i| if i == 23 { 100.0 } else { 1.0 }).collect(),
        }];
        h.text.raw[0] = "SYNTH LED".into();
        h.text.raw[1] = "eulumdat-rs".into();
        h.text.raw[6] = "150mA".into();
        h
    }

    #[test]
    fn axis_mapping_matches_photometric_solid() {
        // Emission axis (+z) lands on Bevy −Y, C = 0 (+x) on +X, C = 90 (+y) on +Z.
        assert_eq!(to_bevy([0.0, 0.0, 1.0]), [0.0, -1.0, 0.0]);
        assert_eq!(to_bevy([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(to_bevy([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(from_bevy(to_bevy([0.3, -0.7, 2.0])), [0.3, -0.7, 2.0]);
        // Proper rotation: (x × y) · z stays +1.
        let (a, b, c) = (
            to_bevy([1.0, 0.0, 0.0]),
            to_bevy([0.0, 1.0, 0.0]),
            to_bevy([0.0, 0.0, 1.0]),
        );
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let det = cross[0] * c[0] + cross[1] * c[1] + cross[2] * c[2];
        assert_eq!(det, 1.0);
    }

    #[test]
    fn c_gamma_convention() {
        let (c, g) = direction_to_c_gamma(0.0, 0.0, 1.0);
        assert_relative_eq!(g, 0.0);
        assert_relative_eq!(c, 0.0);
        let (c, g) = direction_to_c_gamma(1.0, 0.0, 0.0);
        assert_relative_eq!(g, 90.0);
        assert_relative_eq!(c, 0.0);
        let (c, g) = direction_to_c_gamma(0.0, 1.0, 0.0);
        assert_relative_eq!(g, 90.0);
        assert_relative_eq!(c, 90.0);
        let (c, _) = direction_to_c_gamma(0.0, -1.0, 1.0);
        assert_relative_eq!(c, 270.0);
        let (_, g) = direction_to_c_gamma(0.0, 0.0, -1.0);
        assert_relative_eq!(g, 180.0);
    }

    #[test]
    fn round_trip_header_and_rays() {
        let rays = lambertian_rays(1000, 5.1e-5, 1);
        let bytes = to_bytes(&header_uv(), &rays).unwrap();
        // Ray block starts right after the 51-point table + name count.
        assert_eq!(bytes.len(), 36_704 + 1000 * 28);
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.version, 2013);
        assert_eq!(f.header.creation_method, -1);
        assert_eq!(f.header.n_rays, 1000);
        assert_eq!(f.header.ray_start, 36_704);
        assert_eq!(f.header.record_size(), 28);
        assert_eq!(f.header.date_time, "2026-05-12 13:57:44");
        assert_eq!(f.header.spectral_id, SpectralId::SharedTable);
        assert_eq!(f.header.single_wavelength_nm, None);
        assert_eq!(f.header.min_wavelength_nm, Some(220.0));
        assert_eq!(f.header.flux_kind(), FluxKind::Radiometric);
        assert_eq!(f.header.text.name(), "SYNTH LED");
        assert_eq!(f.header.text.operating_condition(), "150mA");
        let t = f.header.shared_spectrum().unwrap();
        assert_eq!(t.len(), 51);
        assert_eq!(t.peak(), Some((266.0, 100.0)));
        assert!(t.is_ultraviolet());
        assert_eq!(f.rays.len(), 1000);
        for (i, r) in f.rays.iter().enumerate() {
            assert_eq!(r, rays[i]);
        }
        assert_eq!(f.rays.get(1000), None);
        // Header re-encodes identically.
        let again = encode_header(&f.header, 1000).unwrap();
        assert_eq!(again, &bytes[..36_704]);
    }

    #[test]
    fn errors_are_descriptive() {
        let rays = lambertian_rays(10, 1.0, 2);
        let bytes = to_bytes(&header_uv(), &rays).unwrap();

        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(matches!(Tm25File::parse(&bad), Err(Tm25Error::BadMagic(_))));

        let mut bad = bytes.clone();
        bad[4..8].copy_from_slice(&2020i32.to_le_bytes());
        assert!(matches!(
            Tm25File::parse(&bad),
            Err(Tm25Error::UnsupportedVersion(2020))
        ));

        assert!(matches!(
            Tm25File::parse(&bytes[..100]),
            Err(Tm25Error::Truncated { .. })
        ));
        assert!(matches!(
            Tm25File::parse(&bytes[..36_500]),
            Err(Tm25Error::Truncated { .. })
        ));

        assert!(matches!(
            Tm25File::parse(&bytes[..bytes.len() - 5]),
            Err(Tm25Error::SizeMismatch {
                ray_start: 36_704,
                ..
            })
        ));

        // No flux column at all is rejected strictly, accepted leniently.
        let mut h = header_uv();
        h.flags.radiant_flux = false;
        h.spectral_id = SpectralId::None;
        h.spectra.clear();
        let plain: Vec<Ray> = rays
            .iter()
            .map(|r| Ray::new(r.position(), r.direction()))
            .collect();
        let b = to_bytes(&h, &plain).unwrap();
        assert!(matches!(
            Tm25File::parse(&b),
            Err(Tm25Error::InvalidHeader(_))
        ));
        let lenient = Tm25File::parse_with(
            &b,
            &ParseOptions {
                strict_flags: false,
            },
        )
        .unwrap();
        assert_eq!(lenient.header.record_size(), 24);
        assert_eq!(lenient.rays.get(0).unwrap().radiant_flux_w, None);

        // Bad UTF-32 in a text field.
        let mut bad = bytes.clone();
        bad[288..292].copy_from_slice(&0x0011_0000u32.to_le_bytes());
        assert!(matches!(
            Tm25File::parse(&bad),
            Err(Tm25Error::InvalidText { field: "name", .. })
        ));
    }

    #[test]
    fn additional_items_shift_the_ray_block() {
        let mut h = header_uv();
        h.n_additional_items = 1;
        h.column_names = vec!["phase".to_string()];
        h.additional_text = "hi".to_string();
        let rays = lambertian_rays(5, 1.0, 3);
        let bytes = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        // count(4) + len(4) + 5 chars (20) + 2 text chars (8) after the table.
        assert_eq!(f.header.ray_start, 36_700 + 4 + 4 + 20 + 8);
        assert_eq!(f.header.record_size(), 32);
        assert_eq!(f.header.columns.last(), Some(&Column::Additional(0)));
        assert_eq!(f.header.column_names, vec!["phase".to_string()]);
        assert_eq!(f.header.additional_text, "hi");
        assert_eq!(f.header.additional_text_size, 2);
        assert_eq!(f.rays.get(4).unwrap().position(), rays[4].position());
        assert_eq!(
            f.rays
                .layout()
                .field(f.rays.raw(0).unwrap(), Column::Additional(0)),
            Some(0.0)
        );
    }

    #[test]
    fn photometric_and_both_layouts() {
        let mut h = Header::new(KnownDataFlags::photometric());
        h.luminous_flux_lm = 100.0;
        let rays: Vec<Ray> = lambertian_rays(3, 1.0, 4)
            .into_iter()
            .map(|r| Ray::new(r.position(), r.direction()).with_luminous_flux(2.5))
            .collect();
        let bytes = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.flux_kind(), FluxKind::Photometric);
        assert_eq!(f.rays.get(0).unwrap().luminous_flux_lm, Some(2.5));
        assert_eq!(f.rays.get(0).unwrap().flux(FluxKind::Photometric), 2.5);

        let mut h = Header::new(KnownDataFlags {
            luminous_flux: true,
            ..KnownDataFlags::radiometric()
        });
        h.radiant_flux_w = 1.0;
        h.luminous_flux_lm = 683.0;
        let rays: Vec<Ray> = rays.iter().map(|r| r.with_radiant_flux(0.01)).collect();
        let bytes = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.flux_kind(), FluxKind::Both);
        assert_eq!(f.header.record_size(), 32);
        assert_eq!(f.header.column_index(Column::LuminousFlux), Some(7));
    }

    #[test]
    fn streaming_reader_matches_slice_reader() {
        let rays = lambertian_rays(10_007, 1e-6, 5);
        let bytes = to_bytes(&header_uv(), &rays).unwrap();
        let slice = Tm25File::parse(&bytes).unwrap();

        // Feed through a Read that returns tiny pieces to exercise buffering.
        struct Trickle<'a>(&'a [u8], usize);
        impl std::io::Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(self.1).min(self.0.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut reader = Tm25Reader::new(Trickle(&bytes, 1000)).unwrap();
        assert_eq!(reader.header(), &slice.header);
        let mut all = Vec::new();
        loop {
            let chunk = reader.read_chunk(999).unwrap();
            if chunk.is_empty() {
                break;
            }
            all.extend(chunk);
        }
        assert_eq!(all.len(), 10_007);
        assert!(all.iter().zip(slice.rays.iter()).all(|(a, b)| *a == b));

        let streamed: Vec<Ray> = Tm25Reader::new(Trickle(&bytes, 4096))
            .unwrap()
            .rays()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(streamed, all);

        // Early EOF is a ray-count error, not a panic.
        let mut short = Tm25Reader::new(Trickle(&bytes[..bytes.len() - 28 * 10], 4096)).unwrap();
        let mut got = 0u64;
        let err = loop {
            match short.read_chunk(4096) {
                Ok(v) if v.is_empty() => panic!("expected an error"),
                Ok(v) => got += v.len() as u64,
                Err(e) => break e,
            }
        };
        assert!(
            matches!(err, Tm25Error::RayCountMismatch { expected: 10_007, actual } if actual >= got)
        );
    }

    #[test]
    fn subsampling_preserves_flux_and_is_deterministic() {
        let rays = lambertian_rays(5000, 1e-5, 6);
        let bytes = to_bytes(&header_uv(), &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        let total: f64 = f
            .rays
            .iter()
            .map(|r| r.radiant_flux_w.unwrap() as f64)
            .sum();

        let a = subsample(&f.rays, 500, 42);
        let b = subsample(&f.rays, 500, 42);
        let c = subsample(&f.rays, 500, 43);
        assert_eq!(a.len(), 500);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let kept: f64 = a.iter().map(|r| r.radiant_flux_w.unwrap() as f64).sum();
        assert_relative_eq!(kept, total, max_relative = 1e-4);
        assert_eq!(subsample(&f.rays, 10_000, 1).len(), 5000);

        let mut res = Reservoir::new(300, 7);
        res.extend(f.rays.iter());
        assert_eq!(res.seen(), 5000);
        let kept = res.finish();
        assert_eq!(kept.len(), 300);
        let sum: f64 = kept.iter().map(|r| r.radiant_flux_w.unwrap() as f64).sum();
        assert_relative_eq!(sum, total, max_relative = 1e-4);
        let mut res2 = Reservoir::new(300, 7);
        res2.extend(f.rays.iter());
        assert_eq!(res2.finish(), kept);
    }

    #[test]
    fn far_field_of_lambertian_is_cosine() {
        let rays = lambertian_rays(400_000, 1e-6, 8);
        let ff = FarField::from_rays(&rays, 10.0, 5.0, FluxKind::Radiometric);
        assert_eq!(ff.unit, IntensityUnit::WattPerSteradian);
        assert_eq!(ff.ray_count, 400_000);
        assert_relative_eq!(ff.total_flux, 0.4, max_relative = 1e-6);
        // Lambertian: I(γ) = Φ/π · cos γ.
        let prof = ff.azimuthal_average();
        let g = ff.g_angles();
        let i0 = 0.4 / std::f64::consts::PI;
        for (gi, &gamma) in g.iter().enumerate() {
            if gamma < 80.0 {
                assert_relative_eq!(prof[gi], i0 * gamma.to_radians().cos(), max_relative = 0.05);
            }
            if gamma > 90.0 {
                assert_eq!(prof[gi], 0.0);
            }
        }
        let half = ff.half_intensity_gamma().unwrap();
        assert!((half - 60.0).abs() < 2.0, "half intensity at {half}");
        assert_relative_eq!(ff.forward_fraction(), 1.0, max_relative = 1e-6);
        // Nearest-bin and bilinear agree at bin centres; bilinear is continuous.
        assert_relative_eq!(ff.sample_nearest(0.0, 0.0), ff.intensity(0, 0));
        assert_relative_eq!(ff.sample_nearest(359.9, 179.9), ff.intensity(35, 35));
        assert_relative_eq!(ff.sample(5.0, 2.5), ff.intensity(0, 0));
        assert_relative_eq!(ff.sample(355.0, 177.5), ff.intensity(35, 35));
        let mid = ff.sample(10.0, 2.5);
        assert!((mid - 0.5 * (ff.intensity(0, 0) + ff.intensity(1, 0))).abs() < 1e-9);
        // Smoothing keeps a rotationally symmetric profile and removes pole noise.
        let sm = ff.smoothed(1, 0, 10.0);
        let p0: Vec<f64> = (0..36).map(|ci| sm.intensity(ci, 0)).collect();
        assert!(p0.iter().all(|v| (v - p0[0]).abs() < 1e-9));
        assert_relative_eq!(sm.azimuthal_average()[10], prof[10], max_relative = 0.02);
        // Single bins near the axis hold only ~85 rays, so the per-bin maximum
        // is noisy; the azimuthal average of the first bin is the robust check.
        assert_relative_eq!(prof[0], i0, max_relative = 0.05);
        assert!(ff.max_intensity() >= prof[0]);
    }
}
