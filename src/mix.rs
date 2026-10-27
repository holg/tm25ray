//! Mixing ray files: several channels of one package (ams OSRAM ships a
//! blue and a phosphor-yellow ray file per white LED, so the user can set
//! the ratio and so the spectra stay adjustable) combined into one TM-25
//! ray set with chosen weights.
//!
//! Two outputs:
//!
//! - [`MixMode::Shared`]: one spectral table (identifier 3), the weighted sum
//!   of the channel spectra on a common 1 nm grid, and every ray's flux
//!   scaled by its channel weight. Reads everywhere a single-spectrum file
//!   reads, and is what a LightTools user wants beside one `.sre`.
//! - [`MixMode::Indexed`]: the channel tables kept (identifier 4) with a
//!   spectrum index per ray, radiometric flux, which is what TM-25 was
//!   designed to do with exactly this case.
//!
//! Spectra are weighted by radiant power. A channel whose rays carry only
//! luminous flux is converted through the ratio of the header totals
//! (`radiant_flux_w / luminous_flux_lm`), which vendor files provide; a
//! channel without any radiometric information cannot be mixed spectrally.
//!
//! [`fit_weights`] finds the weights that reproduce a target spectrum (a
//! vendor's white for one CCT bin, say) from the channel spectra by least
//! squares, which is how the ratio for a given colour is found.

use crate::error::{Result, Tm25Error};
use crate::header::{FluxKind, Header, KnownDataFlags, SpectralId, SpectralTable};
use crate::ray::Ray;

/// How the channel spectra go into the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixMode {
    /// One combined spectral table, spectral identifier 3.
    Shared,
    /// All tables, a spectrum index per ray, spectral identifier 4.
    Indexed,
}

/// One input to the mix.
#[derive(Clone, Debug)]
pub struct Channel<'a> {
    pub header: &'a Header,
    pub rays: &'a [Ray],
    /// Multiplier on every ray flux (1.0 = as in the file).
    pub weight: f64,
    /// Label for the result's description, e.g. `blue`.
    pub label: String,
    /// Spectral table to use instead of the file's own: vendors ship the
    /// blue and yellow spectra per colour bin beside one pair of ray files,
    /// and the bin's spectra must go with the bin's flux split.
    pub spectrum: Option<SpectralTable>,
}

impl<'a> Channel<'a> {
    /// The spectrum this channel contributes: the override, else the
    /// file's shared table.
    pub fn spectrum(&self) -> Option<&SpectralTable> {
        self.spectrum
            .as_ref()
            .or_else(|| self.header.shared_spectrum())
    }
}

/// Linear interpolation of a table at `nm`; 0 outside its range.
pub fn table_value_at(t: &SpectralTable, nm: f32) -> f32 {
    let w = &t.wavelengths_nm;
    if w.is_empty() || nm < w[0] || nm > w[w.len() - 1] {
        return 0.0;
    }
    let i = w.partition_point(|x| *x <= nm);
    if i == 0 {
        return t.values[0];
    }
    if i >= w.len() {
        return t.values[w.len() - 1];
    }
    let (w0, w1) = (w[i - 1], w[i]);
    let (v0, v1) = (t.values[i - 1], t.values[i]);
    if w1 <= w0 {
        return v0;
    }
    v0 + (v1 - v0) * (nm - w0) / (w1 - w0)
}

/// Integral of a table over its range, trapezoidal, in value × nm.
pub fn table_integral(t: &SpectralTable) -> f64 {
    let mut s = 0.0;
    for i in 1..t.len() {
        s += (t.values[i - 1] as f64 + t.values[i] as f64)
            * 0.5
            * (t.wavelengths_nm[i] - t.wavelengths_nm[i - 1]) as f64;
    }
    s
}

/// Weighted sum of spectra on a common 1 nm grid, each table first
/// normalised to unit integral so `powers` are the radiant powers of the
/// channels. The result is scaled to a peak of 100, as vendors write it.
pub fn combine_spectra(tables: &[(&SpectralTable, f64)]) -> Option<SpectralTable> {
    let used: Vec<(&SpectralTable, f64)> = tables
        .iter()
        .filter(|(t, p)| t.len() >= 2 && *p > 0.0)
        .copied()
        .collect();
    if used.is_empty() {
        return None;
    }
    let lo = used
        .iter()
        .map(|(t, _)| t.wavelengths_nm[0])
        .fold(f32::MAX, f32::min)
        .floor();
    let hi = used
        .iter()
        .map(|(t, _)| t.wavelengths_nm[t.len() - 1])
        .fold(f32::MIN, f32::max)
        .ceil();
    let norms: Vec<f64> = used
        .iter()
        .map(|(t, _)| table_integral(t).max(1e-30))
        .collect();
    let mut out = SpectralTable::default();
    let mut nm = lo;
    while nm <= hi + 1e-3 {
        let v: f64 = used
            .iter()
            .zip(&norms)
            .map(|((t, p), n)| table_value_at(t, nm) as f64 / n * p)
            .sum();
        out.wavelengths_nm.push(nm);
        out.values.push(v as f32);
        nm += 1.0;
    }
    let peak = out.values.iter().cloned().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for v in &mut out.values {
            *v *= 100.0 / peak;
        }
    }
    Some(out)
}

/// Watts per lumen for a photometric channel: through the override
/// spectrum's luminous efficacy when one is given (the bin's spectrum
/// decides the watts behind the binned lumens), else the header's ratio of
/// totals. `Some(1.0)` for a channel whose rays carry watts already.
fn watts_per_lumen(c: &Channel) -> Option<f64> {
    let h = c.header;
    if h.flags.radiant_flux {
        return Some(1.0);
    }
    if !h.flags.luminous_flux {
        return None;
    }
    if let Some(ler) = c
        .spectrum
        .as_ref()
        .and_then(crate::photometry::luminous_efficacy_lm_per_w)
    {
        if ler > 0.0 {
            return Some(1.0 / ler);
        }
    }
    if h.radiant_flux_w > 0.0 && h.luminous_flux_lm > 0.0 {
        return Some(h.radiant_flux_w as f64 / h.luminous_flux_lm as f64);
    }
    None
}

/// Radiant power of a channel's rays after weighting.
fn radiant_power(c: &Channel) -> Option<f64> {
    let ratio = watts_per_lumen(c)?;
    let h = c.header;
    if h.flags.radiant_flux {
        return Some(
            c.rays
                .iter()
                .map(|r| r.radiant_flux_w.unwrap_or(0.0) as f64)
                .sum::<f64>()
                * c.weight,
        );
    }
    let lm: f64 = c
        .rays
        .iter()
        .map(|r| r.luminous_flux_lm.unwrap_or(0.0) as f64)
        .sum();
    Some(lm * ratio * c.weight)
}

/// Combine channels into one ray set. Rays are concatenated with their flux
/// scaled by the channel weight; the result is not subsampled.
pub fn mix(channels: &[Channel], mode: MixMode) -> Result<(Header, Vec<Ray>)> {
    if channels.is_empty() {
        return Err(Tm25Error::InvalidHeader("no channels to mix".into()));
    }
    let all_lum = channels.iter().all(|c| c.header.flags.luminous_flux);
    let powers: Vec<Option<f64>> = channels.iter().map(radiant_power).collect();
    let spectra: Vec<Option<&SpectralTable>> = channels.iter().map(|c| c.spectrum()).collect();
    let spectral = spectra.iter().all(|s| s.is_some()) && powers.iter().all(|p| p.is_some());

    // A radiant column can come from the file or from lumens × the header ratio.
    let rad_ratio: Vec<Option<f64>> = channels.iter().map(watts_per_lumen).collect();
    let rad_possible = rad_ratio.iter().all(|r| r.is_some());
    let need_rad = mode == MixMode::Indexed || !all_lum;
    if need_rad && !rad_possible {
        return Err(Tm25Error::InvalidHeader(
            "a channel has neither a radiant flux column nor a radiant total in its header".into(),
        ));
    }
    if mode == MixMode::Indexed && !spectral {
        return Err(Tm25Error::InvalidHeader(
            "an indexed mix needs a spectral table and radiometric information in every channel"
                .into(),
        ));
    }
    let mut flags = KnownDataFlags::radiometric();
    flags.radiant_flux = need_rad || rad_possible;
    flags.luminous_flux = all_lum && mode == MixMode::Shared;
    flags.spectrum_index = mode == MixMode::Indexed;
    if !flags.radiant_flux && !flags.luminous_flux {
        return Err(Tm25Error::InvalidHeader(
            "channels have no common flux column".into(),
        ));
    }

    let mut h = Header::new(flags);
    h.creation_method = 0;
    let first = channels[0].header;
    h.date_time = first.date_time.clone();
    h.text.raw[0] = format!(
        "{} mix: {}",
        first.text.name().trim(),
        channels
            .iter()
            .map(|c| format!("{} × {:.3}", c.label, c.weight))
            .collect::<Vec<_>>()
            .join(" + ")
    );
    h.text.raw[1] = first.text.manufacturer().to_string();
    h.text.raw[2] = first.text.model_creator().to_string();
    h.text.raw[3] = "tm25ray mix".to_string();
    h.text.raw[6] = first.text.operating_condition().to_string();
    h.text.raw[7] = format!(
        "Mixed from {} channel files with weights {}; spectra weighted by radiant power.",
        channels.len(),
        channels
            .iter()
            .map(|c| format!("{:.4}", c.weight))
            .collect::<Vec<_>>()
            .join(", ")
    );
    h.text.raw[8] = first.text.data_reference().to_string();
    h.luminous_flux_lm = if all_lum {
        channels
            .iter()
            .map(|c| c.header.luminous_flux_lm as f64 * c.weight)
            .sum::<f64>() as f32
    } else {
        0.0
    };
    h.radiant_flux_w = powers.iter().flatten().sum::<f64>() as f32;
    let lo = spectra
        .iter()
        .flatten()
        .map(|t| t.wavelengths_nm.first().copied().unwrap_or(f32::MAX))
        .fold(f32::MAX, f32::min);
    let hi = spectra
        .iter()
        .flatten()
        .map(|t| t.wavelengths_nm.last().copied().unwrap_or(f32::MIN))
        .fold(f32::MIN, f32::max);
    match mode {
        MixMode::Shared => {
            if spectral {
                let pairs: Vec<(&SpectralTable, f64)> = spectra
                    .iter()
                    .zip(&powers)
                    .map(|(s, p)| (s.unwrap(), p.unwrap()))
                    .collect();
                if let Some(t) = combine_spectra(&pairs) {
                    h.spectral_id = SpectralId::SharedTable;
                    h.min_wavelength_nm = Some(lo);
                    h.max_wavelength_nm = Some(hi);
                    h.spectra = vec![t];
                }
            }
        }
        MixMode::Indexed => {
            h.spectral_id = SpectralId::PerRayTableIndex;
            h.min_wavelength_nm = Some(lo);
            h.max_wavelength_nm = Some(hi);
            h.spectra = spectra.iter().map(|s| s.unwrap().clone()).collect();
        }
    }
    h.columns = crate::header::columns_from_flags(&h.flags, 0);

    let mut rays = Vec::with_capacity(channels.iter().map(|c| c.rays.len()).sum());
    for (i, c) in channels.iter().enumerate() {
        let w = c.weight as f32;
        let ratio = rad_ratio[i].unwrap_or(1.0) as f32;
        for r in c.rays {
            let mut out = Ray::new(r.position(), r.direction());
            if flags.luminous_flux {
                out.luminous_flux_lm = Some(r.luminous_flux_lm.unwrap_or(0.0) * w);
            }
            if flags.radiant_flux {
                let rad = match r.radiant_flux_w {
                    Some(v) if c.header.flags.radiant_flux => v,
                    _ => r.luminous_flux_lm.unwrap_or(0.0) * ratio,
                };
                out.radiant_flux_w = Some(rad * w);
            }
            if mode == MixMode::Indexed {
                out.spectrum_index = Some(i as u32 + 1);
            }
            rays.push(out);
        }
    }
    Ok((h, rays))
}

/// Least-squares weights `w_i ≥ 0` such that `Σ w_i S_i(λ)` best matches
/// `target(λ)` over the target's range, every spectrum first normalised to
/// unit integral (so the weights are radiant-power fractions). Two or
/// three channels in practice; solved by normal equations with negative
/// weights clamped and re-solved.
pub fn fit_weights(target: &SpectralTable, spectra: &[&SpectralTable]) -> Vec<f64> {
    let n = spectra.len();
    if n == 0 || target.len() < 2 {
        return Vec::new();
    }
    let norms: Vec<f64> = spectra
        .iter()
        .map(|t| table_integral(t).max(1e-30))
        .collect();
    let tnorm = table_integral(target).max(1e-30);
    // Sample on the target's own grid.
    let rows: Vec<(Vec<f64>, f64)> = target
        .wavelengths_nm
        .iter()
        .zip(&target.values)
        .map(|(nm, v)| {
            (
                spectra
                    .iter()
                    .zip(&norms)
                    .map(|(t, k)| table_value_at(t, *nm) as f64 / k)
                    .collect(),
                *v as f64 / tnorm,
            )
        })
        .collect();
    let mut active: Vec<bool> = vec![true; n];
    for _ in 0..n + 1 {
        let idx: Vec<usize> = (0..n).filter(|i| active[*i]).collect();
        if idx.is_empty() {
            return vec![0.0; n];
        }
        let m = idx.len();
        let mut a = vec![vec![0.0f64; m]; m];
        let mut b = vec![0.0f64; m];
        for (x, y) in &rows {
            for (p, &ip) in idx.iter().enumerate() {
                b[p] += x[ip] * y;
                for (q, &iq) in idx.iter().enumerate() {
                    a[p][q] += x[ip] * x[iq];
                }
            }
        }
        let sol = solve(a, b);
        let mut w = vec![0.0; n];
        let mut clamped = false;
        for (p, &ip) in idx.iter().enumerate() {
            if sol[p] < 0.0 {
                active[ip] = false;
                clamped = true;
            } else {
                w[ip] = sol[p];
            }
        }
        if !clamped {
            return w;
        }
    }
    vec![0.0; n]
}

/// Gaussian elimination with partial pivoting for a small dense system.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n)
            .max_by(|i, j| a[*i][col].abs().partial_cmp(&a[*j][col].abs()).unwrap())
            .unwrap_or(col);
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        if d.abs() < 1e-300 {
            continue;
        }
        let pivot_row = a[col].clone();
        for row in col + 1..n {
            let f = a[row][col] / d;
            for (k, v) in a[row].iter_mut().enumerate().skip(col) {
                *v -= f * pivot_row[k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for col in (0..n).rev() {
        let mut s = b[col];
        for k in col + 1..n {
            s -= a[col][k] * x[k];
        }
        x[col] = if a[col][col].abs() < 1e-300 {
            0.0
        } else {
            s / a[col][col]
        };
    }
    x
}

/// Unit of the mixed flux the viewer should quote.
pub fn mixed_flux_kind(h: &Header) -> FluxKind {
    h.flux_kind()
}

/// Weights that give the channels the flux fractions a vendor tabulates
/// per colour bin (ams OSRAM: "photometric yellow 0.95, blue 0.05 for
/// 3000 K"), keeping the summed flux of the inputs: `w_i = f_i × Σflux /
/// flux_i`. `fluxes` are the channels' totals in the basis the fractions
/// use (lm or W); fractions are normalised to 1 first.
pub fn weights_for_flux_fractions(fluxes: &[f64], fractions: &[f64]) -> Vec<f64> {
    let total: f64 = fluxes.iter().sum();
    let fsum: f64 = fractions.iter().sum::<f64>().max(1e-30);
    fluxes
        .iter()
        .zip(fractions)
        .map(|(flux, f)| {
            if *flux > 0.0 {
                f / fsum * total / flux
            } else {
                0.0
            }
        })
        .collect()
}
