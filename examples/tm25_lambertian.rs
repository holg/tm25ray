//! Generate a ground-truth `.TM25RAY` file for checking luminance code.
//!
//!     cargo run --release --example tm25_lambertian -- lambertian.TM25RAY [n_rays]
//!
//! A flat Lambertian emitter is the one source whose luminance is known
//! exactly: `L = Φ / (π A)`, the same at every point of the surface and in
//! every direction. This writes a 1 × 1 mm square in the z = 0 plane, emitting
//! 1 W into the upper hemisphere, so the expected radiance is
//! 1 / (π · 10⁻⁶ m²) = 318 310 W/(sr·m²). Any luminance estimator run on it
//! should return that value, within Monte Carlo noise, at every angle; see
//! `docs/luminance.md` for the method and the noise to expect.
//!
//! Start points are uniform on the square, directions cosine-weighted (uniform
//! in projected solid angle), and every ray carries the same flux. That is the
//! shape Julius Muschaweck, the initiator of TM-25, recommends for a ground
//! truth: equal-power rays keep the Monte-Carlo noise even across angles. Each
//! ray also carries a wavelength drawn uniformly from 360 to 830 nm, so the
//! file has a flat spectrum: the radiance is 318 310 W/(sr·m²), the spectral
//! radiance 677 W/(sr·m²·nm), and a luminance estimator that applies V(λ)
//! should find 318 310 × 683 × V̄ ≈ 49.5 Mcd/m², where V̄ ≈ 0.227 is the mean
//! of V(λ) over 360–830 nm. The wavelength costs 14 % file size.
//!
//! The random numbers come from the crate's xorshift generator; a
//! low-discrepancy (Sobol) sequence would give less noise per ray but needs
//! care in several dimensions, so this stays with plain pseudo-random draws.

use std::f64::consts::PI;

use tm25ray::{write_tm25, Header, KnownDataFlags, Ray, Rng64, SpectralId};

/// Half-width of the emitting square, mm.
const HALF: f64 = 0.5;
/// Radiant flux of the whole file, W.
const TOTAL_FLUX_W: f32 = 1.0;
/// Wavelength range, nm; the spectrum is flat across it.
const LAMBDA_MIN: f64 = 360.0;
const LAMBDA_MAX: f64 = 830.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "lambertian.TM25RAY".to_string());
    let n: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000)
        .clamp(1_000, 20_000_000);

    let mut rng = Rng64::new(20_260_929);
    let flux_each = TOTAL_FLUX_W / n as f32;
    let rays: Vec<Ray> = (0..n)
        .map(|_| {
            let x = (rng.next_f64() * 2.0 - 1.0) * HALF;
            let y = (rng.next_f64() * 2.0 - 1.0) * HALF;
            // Cosine-weighted hemisphere: sin²γ uniform on [0, 1).
            let u = rng.next_f64();
            let phi = 2.0 * PI * rng.next_f64();
            let (sin_g, cos_g) = (u.sqrt(), (1.0 - u).sqrt());
            let lambda = LAMBDA_MIN + (LAMBDA_MAX - LAMBDA_MIN) * rng.next_f64();
            Ray::new(
                [x as f32, y as f32, 0.0],
                [
                    (sin_g * phi.cos()) as f32,
                    (sin_g * phi.sin()) as f32,
                    cos_g as f32,
                ],
            )
            .with_radiant_flux(flux_each)
            .with_wavelength(lambda as f32)
        })
        .collect();

    let area_m2 = (2.0 * HALF) * (2.0 * HALF) * 1e-6;
    let expected = TOTAL_FLUX_W as f64 / (PI * area_m2);
    let expected_spectral = expected / (LAMBDA_MAX - LAMBDA_MIN);

    let mut flags = KnownDataFlags::radiometric();
    flags.wavelength = true;
    let mut h = Header::new(flags);
    // 0 = simulation, which is what this is.
    h.creation_method = 0;
    h.radiant_flux_w = TOTAL_FLUX_W;
    h.date_time = "2026-10-24 12:00:00".into();
    h.spectral_id = SpectralId::PerRayWavelength;
    h.min_wavelength_nm = Some(LAMBDA_MIN as f32);
    h.max_wavelength_nm = Some(LAMBDA_MAX as f32);
    // 4.7.3: name, manufacturer, creator of the optical model, creator of the
    // ray file, equipment / software, camera, operation, additional, geometry.
    h.text.raw[0] = "Lambertian square, 1 x 1 mm, ground truth for luminance".into();
    h.text.raw[1] = "none: analytic emitter, not a product".into();
    h.text.raw[2] = "tm25ray (analytic model)".into();
    h.text.raw[3] = "tm25ray examples/tm25_lambertian.rs".into();
    h.text.raw[4] = "tm25ray writer, generated, not measured".into();
    h.text.raw[6] = "1 W radiant, flat spectrum 360-830 nm".into();
    h.text.raw[7] = format!(
        "Free to copy and redistribute. Flat Lambertian emitter in z = 0, \
         uniform start points, cosine-weighted directions, equal flux per ray, \
         wavelength uniform in [{LAMBDA_MIN:.0}, {LAMBDA_MAX:.0}] nm. Expected \
         radiance {expected:.0} W/(sr*m^2) and spectral radiance \
         {expected_spectral:.1} W/(sr*m^2*nm) at every point and in every direction. \
         See https://github.com/holg/tm25ray/blob/main/docs/luminance.md"
    );
    h.text.raw[8] =
        "emitting square in the z = 0 plane, centred on the origin, emitting into +z".into();

    let file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot write {path}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = write_tm25(std::io::BufWriter::new(file), &h, &rays) {
        eprintln!("write failed: {e}");
        std::process::exit(1);
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "wrote {path}: {n} rays, {:.1} MB, {TOTAL_FLUX_W} W, expected radiance {expected:.0} W/(sr*m^2), \
         spectral radiance {expected_spectral:.1} W/(sr*m^2*nm)",
        size as f64 / 1e6
    );
}
