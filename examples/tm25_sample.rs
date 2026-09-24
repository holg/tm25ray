//! Generate a synthetic `.TM25RAY` file, so anyone can try the format and the
//! viewer without a vendor download.
//!
//!     cargo run --release --example tm25_sample -- sample.TM25RAY [n_rays]
//!
//! Vendor ray files may be used but not redistributed, which leaves people
//! with nothing to open. This writes a file that is honest about what it is —
//! a simulation, not a measurement, and the header says so — while still
//! having the near-field structure that makes a ray file worth having: light
//! leaves a finite die, passes through a dome lens that refracts it, and some
//! of it escapes sideways past the lens rim.
//!
//! The model: a 1.0 × 1.0 mm square chip emitting Lambertian into a hemisphere,
//! covered by a silicone dome of radius 1.4 mm (n = 1.41). Each ray is refracted
//! at the dome surface by Snell's law, with total internal reflection handled by
//! reflecting it back once. Rays that miss the dome leave through a 0.15 mm rim
//! gap, which is what gives the far field its shoulder. Flux is split between a
//! blue peak and a broad phosphor emission, and the per-ray wavelength is drawn
//! from that spectrum, so the viewer's spectral colouring has something to show.
//!
//! Everything is computed from the geometry; nothing is copied from a vendor
//! file. See `docs/format.md` for the layout being written.

use std::f64::consts::PI;

use tm25ray::{write_tm25, Header, KnownDataFlags, Ray, Rng64, SpectralId, SpectralTable};

/// Chip half-width, mm.
const CHIP_HALF: f64 = 0.5;
/// Dome radius, mm.
const DOME_R: f64 = 1.4;
/// Silicone refractive index.
const N_SILICONE: f64 = 1.41;
/// Fraction of rays that escape past the dome rim instead of through it.
const RIM_FRACTION: f64 = 0.06;
/// Radiant flux of the whole file, W.
const TOTAL_FLUX_W: f32 = 1.05;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "sample_white_led.TM25RAY".to_string());
    let n: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(250_000)
        .clamp(1_000, 20_000_000);

    let spectrum = phosphor_white_spectrum();
    let cdf = spectrum_cdf(&spectrum);
    let mut rng = Rng64::new(20_260_924);
    let mut rays = Vec::with_capacity(n);
    let flux_each = TOTAL_FLUX_W / n as f32;

    for _ in 0..n {
        // Start point: uniform on the square chip, at z = 0.
        let ox = (rng.next_f64() * 2.0 - 1.0) * CHIP_HALF;
        let oy = (rng.next_f64() * 2.0 - 1.0) * CHIP_HALF;
        let origin = [ox, oy, 0.0];

        // Lambertian emission into the upper hemisphere.
        let u = rng.next_f64();
        let phi = 2.0 * PI * rng.next_f64();
        let sin_g = u.sqrt();
        let cos_g = (1.0 - u).sqrt();
        let dir = [sin_g * phi.cos(), sin_g * phi.sin(), cos_g];

        let out = if rng.next_f64() < RIM_FRACTION {
            // Escapes through the rim gap: pushed towards the horizon, which
            // is what puts the shoulder in the far field.
            let g = 70f64.to_radians() + rng.next_f64() * 18f64.to_radians();
            let a = 2.0 * PI * rng.next_f64();
            [g.sin() * a.cos(), g.sin() * a.sin(), g.cos()]
        } else {
            refract_through_dome(origin, dir)
        };

        let wl = sample_wavelength(&spectrum, &cdf, rng.next_f64());
        rays.push(
            Ray::new(
                [origin[0] as f32, origin[1] as f32, origin[2] as f32],
                [out[0] as f32, out[1] as f32, out[2] as f32],
            )
            .with_radiant_flux(flux_each)
            .with_wavelength(wl as f32),
        );
    }

    let mut flags = KnownDataFlags::radiometric();
    flags.wavelength = true;
    let mut h = Header::new(flags);
    // 0 = simulation, which is what this is.
    h.creation_method = 0;
    h.radiant_flux_w = TOTAL_FLUX_W;
    h.date_time = "2026-09-24 12:00:00".into();
    h.spectral_id = SpectralId::SharedTable;
    h.min_wavelength_nm = Some(*spectrum.wavelengths_nm.first().unwrap());
    h.max_wavelength_nm = Some(*spectrum.wavelengths_nm.last().unwrap());
    h.spectra = vec![spectrum];
    h.text.raw[0] = "Synthetic white LED, 1 mm chip under a 1.4 mm dome".into();
    h.text.raw[1] = "tm25ray".into();
    h.text.raw[2] = "SAMPLE-WHITE-1".into();
    h.text.raw[3] = "none: this file is generated, not measured".into();
    h.text.raw[4] = "examples/tm25_sample.rs".into();
    h.text.raw[6] = "nominal, 1.05 W radiant".into();
    h.text.raw[7] = "Free to copy and redistribute. Ray-traced from an analytic model \
         (Lambertian chip, Snell refraction at the dome, rim leakage); the \
         numbers are plausible, not a real product's."
        .into();
    h.text.raw[8] = "https://github.com/holg/tm25ray".into();

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
        "wrote {path}: {n} rays, {} B/record, {:.1} MB, {TOTAL_FLUX_W} W",
        h.record_size(),
        size as f64 / 1e6
    );
}

/// Refract a ray leaving the chip at the silicone/air boundary of the dome.
///
/// The dome is a sphere of radius `DOME_R` centred on the chip centre, so the
/// exit point is where the ray meets it and the surface normal is radial.
/// Total internal reflection is handled by reflecting once and re-refracting;
/// if that still fails the ray is left along the surface tangent, which is
/// rare enough not to matter for a demonstration file.
fn refract_through_dome(origin: [f64; 3], dir: [f64; 3]) -> [f64; 3] {
    let Some(hit) = sphere_exit(origin, dir, DOME_R) else {
        return dir;
    };
    let n = normalise(hit);
    match refract(dir, n, N_SILICONE, 1.0) {
        Some(t) => t,
        None => {
            // Total internal reflection: bounce off the dome, try again.
            let r = reflect(dir, n);
            match sphere_exit(hit, r, DOME_R) {
                Some(h2) => {
                    let n2 = normalise(h2);
                    refract(r, n2, N_SILICONE, 1.0).unwrap_or(r)
                }
                None => r,
            }
        }
    }
}

/// Where a ray from `o` in direction `d` leaves a sphere of radius `r`
/// centred at the origin. `None` when it does not (numerically).
fn sphere_exit(o: [f64; 3], d: [f64; 3], r: f64) -> Option<[f64; 3]> {
    let b = 2.0 * dot(o, d);
    let c = dot(o, o) - r * r;
    let disc = b * b - 4.0 * c;
    if disc <= 0.0 {
        return None;
    }
    let t = (-b + disc.sqrt()) / 2.0;
    (t > 0.0).then(|| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t])
}

/// Snell's law at a boundary whose normal is `n`. `None` on total internal
/// reflection.
///
/// The ray is leaving the dome, so it travels *along* the outward normal and
/// `dot(d, n)` is positive; the incidence cosine is that dot product, not its
/// negation. (Getting this backwards sends every ray sideways, which is how
/// the first version of this file ended up with no forward light at all.)
fn refract(d: [f64; 3], n: [f64; 3], n1: f64, n2: f64) -> Option<[f64; 3]> {
    let eta = n1 / n2;
    let cos_i = dot(d, n);
    debug_assert!(cos_i > 0.0, "normal must face the way the ray is going");
    let sin_t2 = eta * eta * (1.0 - cos_i * cos_i);
    if sin_t2 > 1.0 {
        return None;
    }
    let cos_t = (1.0 - sin_t2).sqrt();
    let k = eta * cos_i - cos_t;
    Some(normalise([
        eta * d[0] - k * n[0],
        eta * d[1] - k * n[1],
        eta * d[2] - k * n[2],
    ]))
}

fn reflect(d: [f64; 3], n: [f64; 3]) -> [f64; 3] {
    let k = 2.0 * dot(d, n);
    normalise([d[0] - k * n[0], d[1] - k * n[1], d[2] - k * n[2]])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let n = dot(v, v).sqrt();
    if n <= 0.0 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / n, v[1] / n, v[2] / n]
    }
}

/// A phosphor-converted white LED: a narrow blue pump peak plus the broad
/// yellow-green phosphor band, sampled every 5 nm. Values are percent of peak,
/// which is how vendor files store them.
fn phosphor_white_spectrum() -> SpectralTable {
    let mut wavelengths_nm = Vec::new();
    let mut values = Vec::new();
    let gauss = |x: f64, mu: f64, sigma: f64| (-((x - mu) / sigma).powi(2) / 2.0).exp();
    let mut raw = Vec::new();
    let mut wl = 380.0f64;
    while wl <= 780.0 {
        // Blue pump around 450 nm, phosphor emission around 565 nm with a
        // long red tail.
        let v = 1.00 * gauss(wl, 451.0, 9.0)
            + 0.62 * gauss(wl, 565.0, 52.0)
            + 0.14 * gauss(wl, 625.0, 38.0);
        raw.push(v);
        wavelengths_nm.push(wl as f32);
        wl += 5.0;
    }
    let peak = raw.iter().cloned().fold(0.0f64, f64::max).max(1e-12);
    values.extend(raw.iter().map(|v| (v / peak * 100.0) as f32));
    SpectralTable {
        wavelengths_nm,
        values,
    }
}

/// Cumulative distribution over the spectrum, normalised to 1.
fn spectrum_cdf(t: &SpectralTable) -> Vec<f64> {
    let mut acc = 0.0;
    let mut cdf: Vec<f64> = t
        .values
        .iter()
        .map(|v| {
            acc += *v as f64;
            acc
        })
        .collect();
    if acc > 0.0 {
        for c in &mut cdf {
            *c /= acc;
        }
    }
    if let Some(last) = cdf.last_mut() {
        *last = 1.0;
    }
    cdf
}

/// Draw a wavelength from the spectrum, interpolating inside the chosen bin.
fn sample_wavelength(t: &SpectralTable, cdf: &[f64], u: f64) -> f64 {
    let i = match cdf.binary_search_by(|c| c.partial_cmp(&u).unwrap()) {
        Ok(i) => i,
        Err(i) => i.min(cdf.len() - 1),
    };
    let lo = t.wavelengths_nm[i] as f64;
    let hi = t
        .wavelengths_nm
        .get(i + 1)
        .map(|w| *w as f64)
        .unwrap_or(lo + 5.0);
    lo + (hi - lo) * 0.5
}
