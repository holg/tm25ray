//! IES TM-25 ray files (`.TM25RAY`): zero-copy and streaming readers, a
//! writer, flux-preserving subsampling, a far-field C/γ converter and an
//! export to LightTools binary ray files ([`lighttools`]).
//!
//! TM-25 describes a light source as individual rays (start point in mm,
//! unit direction, flux) rather than an angular intensity table, so it is
//! the near-field counterpart of EULUMDAT/IES LM-63.
//!
//! # Layout (TM-25-13, little-endian)
//!
//! Verified against real vendor files, the reference implementation and
//! Table 4-2 of the standard; see `docs/format.md` in the repository for the
//! full table and the remaining `VERIFY` items.
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
//! | 36 288 | spectral tables (i32 N, N × (f32 λ, f32 value)), zero padded to a multiple of 32 bytes |
//! | then | one 512-byte UTF-32LE name per additional column, then the additional text block (size at offset 84, in bytes) |
//! | then | ray records: `x y z kx ky kz` + flagged columns, f32 except the i32 spectrum index |
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
pub mod lighttools;
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
pub use lighttools::{
    is_lighttools, spectrum_from_rays, to_lighttools_bytes, to_lighttools_bytes_sampled,
    write_lighttools, write_lighttools_sampled, write_sre, LightToolsFile, LightToolsHeader,
    SpectrumSampler, LIGHTTOOLS_VERSION_TAG,
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

    /// A Lumileds-style header: luminous flux, tristimulus columns, unknown
    /// wavelengths written as the signalling NaN, and no spectral table — so
    /// no padding, and the ray block starts right after the fixed 36 288-byte
    /// header.
    fn header_luxeon() -> Header {
        let mut flags = KnownDataFlags::photometric();
        flags.tristimulus = true;
        let mut h = Header::new(flags);
        h.creation_method = 1;
        h.luminous_flux_lm = 23.994_17;
        h.date_time = "2021-03-15T15:50:05-07:00".into();
        h.spectral_id = SpectralId::None;
        h.nan_pattern = 0x7F80_0001;
        h.text.raw[0] = "LUXEON_2835N_2780".into();
        h.text.raw[1] = "Lumileds".into();
        h
    }

    /// With no spectral table there is nothing to pad, so the ray block
    /// starts at 36 288 exactly. A real Lumileds file is laid out like this;
    /// 0.1.0 read its first ray as a column-name count of −1 089 800 634.
    #[test]
    fn header_without_spectral_table_round_trips() {
        let h = header_luxeon();
        let rays: Vec<Ray> = lambertian_rays(2_000, 1.0, 7)
            .into_iter()
            .map(|r| Ray::new(r.position(), r.direction()).with_luminous_flux(0.012))
            .collect();
        let bytes = to_bytes(&h, &rays).unwrap();
        assert_eq!(
            bytes.len(),
            FIXED_HEADER_SIZE + rays.len() * h.record_size()
        );

        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.ray_start, FIXED_HEADER_SIZE);
        assert_eq!(f.header.n_rays, 2_000);
        assert_eq!(f.header.flux_kind(), FluxKind::Photometric);
        assert_eq!(f.header.text.manufacturer(), "Lumileds");
        assert_eq!(f.rays.iter().count(), 2_000);

        // Byte-exact header round trip, including the vendor's NaN sentinel.
        let again = to_bytes(&f.header, &rays).unwrap();
        assert_eq!(&again[..FIXED_HEADER_SIZE], &bytes[..FIXED_HEADER_SIZE]);
        assert_eq!(f.header.nan_pattern, 0x7F80_0001);
    }

    /// Column names and the additional text survive a round trip, and a
    /// header with no rays is a complete file.
    #[test]
    fn column_names_and_text_round_trip() {
        let mut h = header_uv();
        h.n_additional_items = 1;
        h.column_names = vec!["user column".into()];
        h.additional_text = "notes".into();
        let bytes = to_bytes(&h, &[]).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.column_names, vec!["user column".to_string()]);
        assert_eq!(f.header.additional_text, "notes");
        // 5 chars = 20 bytes of text, padded to 32.
        assert_eq!(f.header.additional_text_size, 32);
        assert_eq!(f.header.ray_start, 36_704 + 512 + 32);
        assert_eq!(to_bytes(&f.header, &[]).unwrap(), bytes);
    }

    /// The spectral block is padded to a multiple of 32 bytes: a 51-point
    /// table (412 bytes) gets 4, a 81-point table (652 bytes) gets 20.
    #[test]
    fn spectral_block_is_padded_to_32_bytes() {
        let mut h = header_uv();
        h.spectra[0].wavelengths_nm = (0..81).map(|i| 380.0 + 5.0 * i as f32).collect();
        h.spectra[0].values = vec![1.0; 81];
        h.min_wavelength_nm = Some(380.0);
        h.max_wavelength_nm = Some(780.0);
        let rays = lambertian_rays(3, 1.0, 1);
        let bytes = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.header.ray_start, FIXED_HEADER_SIZE + 652 + 20);
        assert!(bytes[FIXED_HEADER_SIZE + 652..f.header.ray_start]
            .iter()
            .all(|b| *b == 0));
        assert_eq!(f.rays.get(2).unwrap().position(), rays[2].position());
    }

    /// LightTools export: radiometric with a wavelength per ray keeps the
    /// 32-byte record, and the file reads back with the same rays.
    #[test]
    fn lighttools_round_trip_spectral() {
        let mut flags = KnownDataFlags::radiometric();
        flags.wavelength = true;
        let mut h = Header::new(flags);
        h.spectral_id = SpectralId::PerRayWavelength;
        h.min_wavelength_nm = Some(400.0);
        h.max_wavelength_nm = Some(700.0);
        h.radiant_flux_w = 1.0;
        let rays: Vec<Ray> = lambertian_rays(500, 0.002, 9)
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.with_wavelength(400.0 + (i % 300) as f32))
            .collect();
        let bytes = to_lighttools_bytes(&h, &rays).unwrap();
        assert_eq!(&bytes[..4], b"LTRF");
        assert_eq!(bytes.len(), 44 + 500 * 32 + 7);
        assert_eq!(&bytes[bytes.len() - 7..], b"LTRFEND");
        // Header words: major 2, minor 0, radiometric 0, near field 0, colour 2, mm 1.
        let w = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        assert_eq!([w(4), w(8), w(12), w(16), w(20), w(24)], [2, 0, 0, 0, 2, 1]);
        assert_eq!(f32::from_bits(w(40)), 1.0);
        let back = LightToolsFile::parse(&bytes).unwrap();
        assert_eq!(back.header.flux_kind, FluxKind::Radiometric);
        assert!(back.header.per_ray_wavelength);
        assert_eq!(back.rays.len(), 500);
        for (a, b) in back.rays.iter().zip(&rays) {
            assert_eq!(a.position(), b.position());
            assert_eq!(a.direction(), b.direction());
            assert_eq!(a.radiant_flux_w, b.radiant_flux_w);
            assert_eq!(a.wavelength_nm, b.wavelength_nm);
        }
    }

    /// A photometric file exports luminous flux in 28-byte records, and a
    /// flux-rescaled subsample keeps the header total meaningful.
    #[test]
    fn lighttools_photometric_and_subsample() {
        let h = header_luxeon();
        let rays: Vec<Ray> = lambertian_rays(2_000, 1.0, 7)
            .into_iter()
            .map(|r| Ray::new(r.position(), r.direction()).with_luminous_flux(0.012))
            .collect();
        let tm25 = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&tm25).unwrap();
        let mut lt = LightToolsHeader::from_tm25(&f.header).unwrap();
        assert_eq!(lt.flux_kind, FluxKind::Photometric);
        assert!(!lt.per_ray_wavelength);
        lt.total_flux = 2_000.0 * 0.012;
        let mut out = Vec::new();
        let n = write_lighttools(&mut out, &lt, subsample(&f.rays, 200, 3)).unwrap();
        assert_eq!(n, 200);
        assert_eq!(out.len(), 44 + 200 * 28 + 7);
        let back = LightToolsFile::parse(&out).unwrap();
        let sum: f32 = back.rays.iter().map(|r| r.luminous_flux_lm.unwrap()).sum();
        assert!((sum - 24.0).abs() < 1e-3, "{sum}");
        assert!(back.rays.iter().all(|r| r.radiant_flux_w.is_none()));
    }

    /// A LightTools file read back as a TM-25 header streams through the
    /// TM-25 record decoder unchanged: same columns, record size, start
    /// offset and ray count, and written out as TM-25 it parses again with
    /// the same rays.
    #[test]
    fn lighttools_reads_back_as_tm25() {
        let mut flags = KnownDataFlags::radiometric();
        flags.wavelength = true;
        let mut h = Header::new(flags);
        h.spectral_id = SpectralId::PerRayWavelength;
        h.min_wavelength_nm = Some(400.0);
        h.max_wavelength_nm = Some(700.0);
        h.radiant_flux_w = 0.5;
        let rays: Vec<Ray> = lambertian_rays(300, 0.5 / 300.0, 11)
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.with_wavelength(450.0 + i as f32))
            .collect();
        let lt_bytes = to_lighttools_bytes(&h, &rays).unwrap();
        assert!(is_lighttools(&lt_bytes));
        let lt = LightToolsHeader::parse(&lt_bytes).unwrap();
        let th = lt.to_tm25_header("lamp.ray", Some(lt_bytes.len() as u64));
        assert!(th.is_lighttools());
        assert_eq!(th.n_rays, 300);
        assert_eq!(th.ray_start, 44);
        assert_eq!(th.record_size(), 32);
        assert_eq!(th.spectral_id, SpectralId::PerRayWavelength);
        assert_eq!(th.flux_kind(), FluxKind::Radiometric);
        assert_eq!(th.radiant_flux_w, 0.5);
        assert_eq!(th.text.name(), "lamp.ray");
        // Stream the records with the TM-25 layout.
        let layout = RayLayout::from_header(&th);
        let block = &lt_bytes[th.ray_start..th.ray_start + th.ray_block_size() as usize];
        let streamed: Vec<Ray> = block
            .chunks_exact(th.record_size())
            .map(|r| layout.decode(r))
            .collect();
        assert_eq!(streamed, rays);
        // And as a TM-25 file.
        let (th2, rays2) = LightToolsFile::parse(&lt_bytes)
            .unwrap()
            .to_tm25("lamp.ray");
        assert_eq!(th2, th);
        let tm25 = to_bytes(&th2, &rays2).unwrap();
        let f = Tm25File::parse(&tm25).unwrap();
        assert_eq!(f.header.version, 2013);
        assert_eq!(f.rays.iter().collect::<Vec<_>>(), rays);
        // Photometric, no wavelength: 28-byte records, luminous column.
        let lt_p = LightToolsHeader {
            flux_kind: FluxKind::Photometric,
            per_ray_wavelength: false,
            total_flux: 12.0,
            ..lt.clone()
        };
        let th_p = lt_p.to_tm25_header("p.ray", Some(lt_p.file_size_for(10)));
        assert_eq!((th_p.n_rays, th_p.record_size()), (10, 28));
        assert_eq!(th_p.flux_kind(), FluxKind::Photometric);
        assert_eq!(th_p.luminous_flux_lm, 12.0);
        assert_eq!(th_p.spectral_id, SpectralId::None);
    }

    /// `.sre` has the vendor layout; the sampler reproduces the table; the
    /// sampled export carries a wavelength per ray inside the table's range;
    /// the ray histogram recovers the spectrum from such rays.
    #[test]
    fn lighttools_spectrum_companions() {
        let h = header_uv(); // 51 points 220..320 nm, peak 100 at 266 nm
        let sre = write_sre(h.shared_spectrum().unwrap(), "UV-C sample");
        let lines: Vec<&str> = sre.split("\r\n").collect();
        assert_eq!(lines[0], "#\tUV-C sample");
        assert_eq!(lines[1], "dfat 1.0\t");
        assert_eq!(lines[2], "dataname:\tUV-C sample");
        assert_eq!(lines[3], "continuous\t");
        assert_eq!(lines[4], "radiometric\t");
        assert_eq!(lines[5], "220.0000\t0.0100");
        assert!(lines.iter().any(|l| *l == "266.0000\t1.0000"));
        assert_eq!(lines.len(), 5 + 51 + 1);

        // Sampler: a table with two equal triangles must split 50/50 and
        // stay inside the range; a flat table is uniform.
        let flat = SpectralTable {
            wavelengths_nm: vec![400.0, 700.0],
            values: vec![1.0, 1.0],
        };
        let s = SpectrumSampler::new(&flat).unwrap();
        assert!((s.sample(0.0) - 400.0).abs() < 1e-3);
        assert!((s.sample(0.5) - 550.0).abs() < 1e-3);
        let peaked = SpectralTable {
            wavelengths_nm: vec![400.0, 450.0, 500.0],
            values: vec![0.0, 1.0, 0.0],
        };
        let s = SpectrumSampler::new(&peaked).unwrap();
        let mut rng = Rng64::new(5);
        let n = 100_000;
        let below = (0..n).filter(|_| s.sample(rng.next_f64()) < 450.0).count();
        assert!((below as f64 / n as f64 - 0.5).abs() < 0.01, "{below}");
        // Linear density 0→1 on [400, 450]: median at 400 + 50/√2.
        let mut xs: Vec<f32> = (0..n).map(|_| s.sample(rng.next_f64() * 0.5)).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            (xs[n / 2] - (400.0 + 50.0 / 2f32.sqrt())).abs() < 0.5,
            "{}",
            xs[n / 2]
        );

        // Sampled export of the UV header: 32-byte records, λ within range.
        let rays = lambertian_rays(2_000, 0.051 / 2_000.0, 3);
        let bytes = to_lighttools_bytes_sampled(&h, &rays, 1).unwrap();
        let f = LightToolsFile::parse(&bytes).unwrap();
        assert!(f.header.per_ray_wavelength);
        assert_eq!(f.rays.len(), 2_000);
        assert!(f
            .rays
            .iter()
            .all(|r| (220.0..=320.0).contains(&r.wavelength_nm.unwrap())));
        assert_eq!(to_lighttools_bytes_sampled(&h, &rays, 1).unwrap(), bytes);
        // Histogram of those rays peaks where the table does.
        let hist = spectrum_from_rays(&f.rays, FluxKind::Radiometric, 4.0).unwrap();
        assert_eq!(hist.peak().map(|p| p.0.round()), Some(266.0));
        // No table, or photometric only: refused.
        assert!(to_lighttools_bytes_sampled(&header_luxeon(), &rays, 1).is_err());
    }

    /// LightTools has no photometric + spectral combination, and a shared
    /// spectral table is not a per-ray column, so it maps to "no colour".
    #[test]
    fn lighttools_mapping_rules() {
        let h = header_uv();
        let lt = LightToolsHeader::from_tm25(&h).unwrap();
        assert_eq!(lt.flux_kind, FluxKind::Radiometric);
        assert!(!lt.per_ray_wavelength);
        let mut flags = KnownDataFlags::photometric();
        flags.wavelength = true;
        let mut bad = Header::new(flags);
        bad.spectral_id = SpectralId::PerRayWavelength;
        assert!(LightToolsHeader::from_tm25(&bad).is_err());
        let mut none = Header::new(KnownDataFlags::radiometric());
        none.flags.radiant_flux = false;
        assert!(LightToolsHeader::from_tm25(&none).is_err());
        assert!(matches!(
            LightToolsFile::parse(b"LTRX"),
            Err(Tm25Error::Truncated { .. })
        ));
        let mut v3 = to_lighttools_bytes(&h, &[]).unwrap();
        v3[4] = 3;
        assert!(matches!(
            LightToolsFile::parse(&v3),
            Err(Tm25Error::UnsupportedVersion(3))
        ));
    }

    /// The spectrum index is an i32 on disk; the f32 bit pattern the C++
    /// reference writes is accepted too.
    #[test]
    fn spectrum_index_is_an_integer_on_disk() {
        let mut flags = KnownDataFlags::radiometric();
        flags.spectrum_index = true;
        let mut h = Header::new(flags);
        h.spectral_id = SpectralId::PerRayTableIndex;
        h.min_wavelength_nm = Some(400.0);
        h.max_wavelength_nm = Some(700.0);
        let table = |peak: f32| SpectralTable {
            wavelengths_nm: vec![400.0, peak, 700.0],
            values: vec![0.0, 1.0, 0.0],
        };
        h.spectra = vec![table(450.0), table(600.0)];
        h.radiant_flux_w = 2.0;
        let rays = vec![
            Ray::new([0.0; 3], [0.0, 0.0, 1.0])
                .with_radiant_flux(1.0)
                .with_spectrum_index(1),
            Ray::new([0.0; 3], [0.0, 0.0, 1.0])
                .with_radiant_flux(1.0)
                .with_spectrum_index(2),
        ];
        let mut bytes = to_bytes(&h, &rays).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        let start = f.header.ray_start;
        assert_eq!(f.header.record_size(), 32);
        assert_eq!(&bytes[start + 28..start + 32], &1i32.to_le_bytes());
        assert_eq!(f.rays.get(1).unwrap().spectrum_index, Some(2));
        // The reference's float encoding of the same index.
        bytes[start + 60..start + 64].copy_from_slice(&2.0f32.to_le_bytes());
        let f = Tm25File::parse(&bytes).unwrap();
        assert_eq!(f.rays.get(1).unwrap().spectrum_index, Some(2));
    }

    /// Unknown flux totals are NaN in the file; callers get 0.0 so arithmetic
    /// does not poison every derived value. Lumileds writes that sentinel
    /// byte-swapped, so little-endian it decodes to the denormal 2.36e-38
    /// rather than a NaN — which would look like a real, tiny measurement.
    #[test]
    fn unknown_flux_totals_are_normalised() {
        // Both spellings of "unknown": a plain NaN and the swapped sentinel.
        for bits in [0x7FC0_0001u32, 0x0100_807F] {
            let mut h = header_luxeon();
            h.luminous_flux_lm = f32::from_bits(bits);
            h.radiant_flux_w = f32::from_bits(bits);
            let bytes = to_bytes(&h, &[]).unwrap();
            let f = Tm25File::parse(&bytes).unwrap();
            assert_eq!(f.header.luminous_flux_lm, 0.0, "bits {bits:#010x}");
            assert_eq!(f.header.radiant_flux_w, 0.0, "bits {bits:#010x}");
            assert!(!f.header.total_flux(FluxKind::Photometric).is_nan());
        }
    }

    /// A normalised sentinel is written back unchanged, but an edit wins.
    #[test]
    fn unknown_sentinel_survives_a_round_trip_unless_edited() {
        let mut h = header_luxeon();
        h.radiant_flux_w = f32::from_bits(0x0100_807F);
        let bytes = to_bytes(&h, &[]).unwrap();
        let parsed = Tm25File::parse(&bytes).unwrap().header;
        assert_eq!(parsed.radiant_flux_w, 0.0);
        // Untouched: the original sentinel bits come back.
        let again = to_bytes(&parsed, &[]).unwrap();
        assert_eq!(&again[16..20], &0x0100_807Fu32.to_le_bytes());
        // Edited: the new value is written instead.
        let mut edited = parsed.clone();
        edited.radiant_flux_w = 0.051;
        let out = to_bytes(&edited, &[]).unwrap();
        assert_eq!(&out[16..20], &0.051f32.to_bits().to_le_bytes());
    }

    /// The sentinel is matched exactly. Ordinary measurements must survive,
    /// including ones whose byte-swapped image happens to look like a NaN.
    #[test]
    fn real_values_are_not_mistaken_for_unknown() {
        let mut h = header_luxeon();
        h.luminous_flux_lm = 23.994_17;
        h.radiant_flux_w = 0.051;
        h.min_wavelength_nm = Some(220.0);
        h.max_wavelength_nm = Some(320.0);
        let bytes = to_bytes(&h, &[]).unwrap();
        let f = Tm25File::parse(&bytes).unwrap();
        assert_relative_eq!(f.header.luminous_flux_lm, 23.994_17);
        assert_relative_eq!(f.header.radiant_flux_w, 0.051);
        assert_eq!(f.header.min_wavelength_nm, Some(220.0));

        // A value whose swapped bytes are a NaN pattern is still a real value.
        let tricky = f32::from_bits(0x0100_C07F);
        assert!(!tricky.is_nan());
        let mut h2 = header_luxeon();
        h2.luminous_flux_lm = tricky;
        let b2 = to_bytes(&h2, &[]).unwrap();
        let f2 = Tm25File::parse(&b2).unwrap();
        assert_eq!(f2.header.luminous_flux_lm.to_bits(), 0x0100_C07F);
    }

    /// Builder methods for the optional per-ray columns write and read back.
    #[test]
    fn wavelength_and_spectrum_index_round_trip() {
        let mut flags = KnownDataFlags::radiometric();
        flags.wavelength = true;
        let mut h = Header::new(flags);
        h.radiant_flux_w = 1.0;
        let rays: Vec<Ray> = (0..64)
            .map(|i| {
                Ray::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0])
                    .with_radiant_flux(1.0 / 64.0)
                    .with_wavelength(400.0 + i as f32)
            })
            .collect();
        let bytes = to_bytes(&h, &rays).unwrap();
        let back = Tm25File::parse(&bytes).unwrap();
        let decoded: Vec<Ray> = back.rays.iter().collect();
        assert_eq!(decoded.len(), 64);
        assert_eq!(decoded[0].wavelength_nm, Some(400.0));
        assert_eq!(decoded[63].wavelength_nm, Some(463.0));
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
        // 51-point table = 412 bytes, padded to 416: the ray block starts at
        // 36 704, as in the ams OSRAM files.
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
                ..Default::default()
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
        // 4 bytes of padding after the table, a 512-byte name, 2 text chars
        // (8 bytes) padded to 32.
        assert_eq!(f.header.ray_start, 36_700 + 4 + 512 + 32);
        assert_eq!(f.header.record_size(), 32);
        assert_eq!(f.header.columns.last(), Some(&Column::Additional(0)));
        assert_eq!(f.header.column_names, vec!["phase".to_string()]);
        assert_eq!(f.header.additional_text, "hi");
        assert_eq!(f.header.additional_text_size, 32);
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
