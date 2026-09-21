//! Checks against the real ams OSRAM UV-C LED ray files. The vendor's terms
//! grant use, not redistribution, so the files are not in the repository:
//! set `TM25RAY_FIXTURE` to the 100k file (or keep the vendor package
//! in `~/Downloads`) and the test runs; otherwise it prints a notice and
//! passes.

use std::path::PathBuf;

use tm25ray::{FarField, FluxKind, SpectralId, Tm25File, Tm25Reader};

fn fixture() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TM25RAY_FIXTURE") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join(
        "Downloads/rayfile_SU_CULCP1_VC_20260512_IES_TM25/rayfile_SU_CULCP1_VC_100k_20260512_IES_TM25.TM25RAY",
    );
    p.exists().then_some(p)
}

#[test]
fn ams_osram_uvc_100k() {
    let Some(path) = fixture() else {
        eprintln!("TM-25 fixture not found; set TM25RAY_FIXTURE to run this test");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 2_836_704);
    let f = Tm25File::parse(&bytes).expect("parse vendor file");
    let h = &f.header;

    assert_eq!(h.version, 2013);
    assert_eq!(h.creation_method, -1);
    assert_eq!(h.n_rays, 100_000);
    assert_eq!(h.luminous_flux_lm, 0.0);
    assert!((h.radiant_flux_w - 0.051).abs() < 1e-6);
    assert_eq!(h.date_time, "2026-05-12 13:57:44");
    assert_eq!(h.start_position, 0);
    assert_eq!(h.spectral_id, SpectralId::SharedTable);
    assert_eq!(h.single_wavelength_nm, None);
    assert_eq!(h.min_wavelength_nm, Some(220.0));
    assert_eq!(h.max_wavelength_nm, Some(320.0));
    assert_eq!(h.spectra.len(), 1);
    assert_eq!(h.n_additional_items, 0);
    assert_eq!(h.additional_text_size, 0);
    assert!(h.flags.position && h.flags.direction && h.flags.radiant_flux);
    assert!(!h.flags.wavelength && !h.flags.luminous_flux && !h.flags.stokes);
    assert!(!h.flags.tristimulus && !h.flags.spectrum_index);
    assert_eq!(h.flux_kind(), FluxKind::Radiometric);
    assert_eq!(h.record_size(), 28);
    assert_eq!(h.ray_start, 36_704);

    assert_eq!(h.text.name(), "SU CULCP1.VC");
    assert_eq!(h.text.manufacturer(), "ams-OSRAM AG");
    assert_eq!(h.text.operating_condition(), "150mA");
    assert_eq!(h.text.data_reference(), "See Information In PDF file");

    let t = h.shared_spectrum().unwrap();
    assert_eq!(t.len(), 51);
    assert_eq!(t.wavelengths_nm[0], 220.0);
    assert_eq!(*t.wavelengths_nm.last().unwrap(), 320.0);
    assert_eq!(t.peak(), Some((266.0, 100.0)));
    let above_half: Vec<f32> = t
        .wavelengths_nm
        .iter()
        .zip(&t.values)
        .filter(|(_, v)| **v >= 50.0)
        .map(|(w, _)| *w)
        .collect();
    assert_eq!(above_half.first(), Some(&262.0));
    assert_eq!(above_half.last(), Some(&272.0));
    assert!(t.is_ultraviolet());
    // The package ships the same table as ASCII next to the ray files: the
    // embedded copy is in percent (peak 100), the ASCII file is peak-normalised
    // to 1.0 with four decimals.
    if let Ok(txt) =
        std::fs::read_to_string(path.with_file_name("SU_CULCP1_VC_20260512_spectrum.txt"))
    {
        let rows: Vec<(f32, f32)> = txt
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
            })
            .collect();
        assert_eq!(rows.len(), 51);
        for (i, (w, v)) in rows.iter().enumerate() {
            assert!((t.wavelengths_nm[i] - w).abs() < 1e-3);
            assert!(
                (t.values[i] - v * 100.0).abs() < 0.01,
                "{w} nm: {} vs {}",
                t.values[i],
                v * 100.0
            );
        }
    }

    // Rays --------------------------------------------------------------
    assert_eq!(f.rays.len(), 100_000);
    let first = f.rays.get(0).unwrap();
    assert!((first.x - 0.39076).abs() < 1e-4);
    assert!((first.y - 0.12505).abs() < 1e-4);
    assert!((first.z + 0.755).abs() < 1e-5);
    assert!((first.kx + 0.51988).abs() < 1e-4);
    assert!((first.ky - 0.14747).abs() < 1e-4);
    assert!((first.kz - 0.84141).abs() < 1e-4);
    assert_eq!(first.radiant_flux_w, Some(5.1e-7));

    let mut sum = 0.0f64;
    let (mut xmin, mut xmax, mut ymin, mut ymax, mut zmin, mut zmax) =
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    let mut kz_min = f32::MAX;
    for r in f.rays.iter() {
        assert_eq!(r.radiant_flux_w, Some(5.1e-7));
        assert!((r.direction_norm() - 1.0).abs() < 1e-4);
        sum += r.radiant_flux_w.unwrap() as f64;
        xmin = xmin.min(r.x);
        xmax = xmax.max(r.x);
        ymin = ymin.min(r.y);
        ymax = ymax.max(r.y);
        zmin = zmin.min(r.z);
        zmax = zmax.max(r.z);
        kz_min = kz_min.min(r.kz);
    }
    assert!((sum - 0.051).abs() < 1e-5, "flux sum {sum}");
    assert!(xmin >= -1.76 && xmax <= 1.62, "x range {xmin}..{xmax}");
    assert!(ymin >= -1.62 && ymax <= 1.65, "y range {ymin}..{ymax}");
    assert!(zmin >= -0.816 && zmax <= 1e-6, "z range {zmin}..{zmax}");
    assert!((0.16..0.17).contains(&kz_min), "kz min {kz_min}");

    // Far field: flat to ~25°, half intensity near 57°, nothing past ~82°.
    let rays: Vec<_> = f.rays.iter().collect();
    let ff = FarField::from_rays(&rays, 10.0, 10.0, FluxKind::Radiometric);
    let prof = ff.azimuthal_average();
    // Normalise by the profile peak (single bins are Monte Carlo noisy).
    let max = prof.iter().cloned().fold(0.0, f64::max);
    let rel: Vec<f64> = prof.iter().map(|v| v / max).collect();
    let expect = [0.99, 1.00, 0.98, 0.89, 0.72, 0.54, 0.30, 0.09, 0.00];
    for (i, e) in expect.iter().enumerate() {
        assert!((rel[i] - e).abs() < 0.08, "γ bin {i}: {:.2} vs {e}", rel[i]);
    }
    let half = ff.half_intensity_gamma().unwrap();
    assert!((52.0..=62.0).contains(&half), "half intensity at {half}");
    assert!(rel[9..].iter().all(|v| *v == 0.0));
    assert!((max - 0.019).abs() < 0.003, "peak {max} W/sr");
    assert!((ff.forward_fraction() - 1.0).abs() < 1e-6);

    // Streaming path agrees.
    let file = std::fs::File::open(&path).unwrap();
    let mut reader = Tm25Reader::new(std::io::BufReader::new(file)).unwrap();
    assert_eq!(reader.header(), h);
    let mut n = 0usize;
    loop {
        let chunk = reader.read_chunk(10_000).unwrap();
        if chunk.is_empty() {
            break;
        }
        for (i, r) in chunk.iter().enumerate() {
            assert_eq!(*r, rays[n + i]);
        }
        n += chunk.len();
    }
    assert_eq!(n, 100_000);
}

#[cfg(feature = "mmap")]
#[test]
fn ams_osram_uvc_mmap() {
    let Some(path) = fixture() else {
        return;
    };
    let m = tm25ray::Tm25Mmap::open(&path).unwrap();
    let f = m.file().unwrap();
    assert_eq!(f.rays.len(), 100_000);
    assert_eq!(f.rays.bytes().len(), 2_800_000);
}
