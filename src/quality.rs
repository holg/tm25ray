//! Ray-file quality report: is this file usable, and what is wrong with it?
//!
//! A [`QualityAccumulator`] sees every ray once (it fits any streaming
//! pass, the viewer's included) and [`QualityAccumulator::finish`] turns the
//! counts into a [`QualityReport`]: numeric statistics plus graded
//! [`Finding`]s with a short code and a sentence each. Checks:
//!
//! - **Flux**: ray fluxes sum to the header total; no negative, NaN or
//!   zero-flux rays; outliers (a ray carrying far more than its share).
//! - **Directions**: unit length within tolerance, no NaN; the share of rays
//!   pointing into the lower hemisphere (information, not an error: some
//!   packages emit sideways and backwards).
//! - **Start points**: no NaN; the bounding box; start points far outside
//!   the cloud of the others (likely garbage or a mis-scaled unit), judged
//!   against the running centroid after a 1000-ray warm-up.
//! - **Duplicates**: identical records (position, direction and flux), which
//!   hint at a copy-and-append rather than a measurement. Checked on the
//!   first [`DUPLICATE_CHECK_LIMIT`] rays to bound memory.
//! - **Wavelengths**: inside the header bounds for a wavelength per ray;
//!   spectrum indices inside the table count.
//! - **Header**: unknown totals, empty description, non-standard creation
//!   method, missing date.
//! - **Statistics**: a Monte-Carlo noise estimate for the far field from the
//!   ray count, so the user knows what a 10° × 5° bin is worth.
//!
//! The report has no opinion about the physics beyond consistency; a file
//! can pass every check and still be a poor model of the LED.

use std::collections::HashSet;

use crate::header::{FluxKind, Header, SpectralId};
use crate::ray::Ray;

/// Rays whose exact records are remembered for the duplicate check.
pub const DUPLICATE_CHECK_LIMIT: usize = 4_000_000;
/// Direction vectors are "unit" when `| |k| − 1 |` is at most this.
pub const UNIT_TOLERANCE: f32 = 1e-3;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing; nothing to fix.
    Info,
    /// Will distort results in some uses; look before relying on the file.
    Warning,
    /// The file is inconsistent or carries unusable records.
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One observation about the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    /// Stable machine-readable code, e.g. `flux-mismatch`.
    pub code: &'static str,
    pub message: String,
}

/// Numeric statistics gathered over every ray.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct QualityStats {
    pub rays: u64,
    /// Sum of the ray fluxes in the header's unit (W or lm).
    pub flux_sum: f64,
    pub flux_header: f64,
    pub flux_min: f32,
    pub flux_max: f32,
    pub zero_flux: u64,
    pub negative_flux: u64,
    pub nan_flux: u64,
    /// Rays carrying more than 100 × the mean flux.
    pub flux_outliers: u64,
    pub nan_direction: u64,
    pub non_unit_direction: u64,
    /// Largest `| |k| − 1 |` seen over finite directions.
    pub max_unit_error: f32,
    pub backward: u64,
    pub nan_position: u64,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Start points farther from the centroid than 4 × the RMS distance.
    pub position_outliers: u64,
    pub duplicates: u64,
    pub duplicate_check_rays: u64,
    pub nan_wavelength: u64,
    pub wavelength_out_of_bounds: u64,
    pub spectrum_index_out_of_range: u64,
}

/// The finished report.
#[derive(Clone, Debug, PartialEq)]
pub struct QualityReport {
    pub stats: QualityStats,
    pub findings: Vec<Finding>,
    /// Expected relative Monte-Carlo noise of a 10° × 5° far-field bin at
    /// γ = 30°, `1 / √n` with `n` the rays such a bin receives from a
    /// Lambertian-like source.
    pub far_field_noise: f64,
}

impl QualityReport {
    pub fn worst(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }

    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// One line: `OK`, `2 warnings`, `1 error, 3 warnings`.
    pub fn verdict(&self) -> String {
        let (e, w) = (self.errors(), self.warnings());
        match (e, w) {
            (0, 0) => "OK".to_string(),
            (0, w) => format!("{w} warning{}", if w == 1 { "" } else { "s" }),
            (e, 0) => format!("{e} error{}", if e == 1 { "" } else { "s" }),
            (e, w) => format!(
                "{e} error{}, {w} warning{}",
                if e == 1 { "" } else { "s" },
                if w == 1 { "" } else { "s" }
            ),
        }
    }

    /// Plain-text report for a file or a download.
    pub fn to_text(&self, name: &str) -> String {
        let s = &self.stats;
        let mut t = String::new();
        t.push_str(&format!(
            "TM-25 ray file quality report: {name}\nVerdict: {}\n\n",
            self.verdict()
        ));
        for f in &self.findings {
            t.push_str(&format!(
                "[{}] {}: {}\n",
                f.severity.as_str().to_uppercase(),
                f.code,
                f.message
            ));
        }
        if self.findings.is_empty() {
            t.push_str("No findings.\n");
        }
        t.push_str(&format!(
            "\nRays {}\nFlux sum {:.6} vs header {:.6}\nFlux per ray min {:.3e} max {:.3e}\n\
             Direction |k| max error {:.2e}, non-unit {}, backward {} ({:.2} %)\n\
             Start points x [{:.4}, {:.4}] y [{:.4}, {:.4}] z [{:.4}, {:.4}] mm, outliers {}\n\
             Duplicates {} (checked {} rays)\n\
             Far-field noise estimate: {:.1} % per 10° × 5° bin at γ = 30°\n",
            s.rays,
            s.flux_sum,
            s.flux_header,
            s.flux_min,
            s.flux_max,
            s.max_unit_error,
            s.non_unit_direction,
            s.backward,
            if s.rays > 0 {
                s.backward as f64 / s.rays as f64 * 100.0
            } else {
                0.0
            },
            s.min[0],
            s.max[0],
            s.min[1],
            s.max[1],
            s.min[2],
            s.max[2],
            s.position_outliers,
            s.duplicates,
            s.duplicate_check_rays,
            self.far_field_noise * 100.0
        ));
        t
    }
}

/// Streaming collector; feed every ray, then [`finish`](Self::finish).
#[derive(Clone, Debug)]
pub struct QualityAccumulator {
    kind: FluxKind,
    header: Header,
    s: QualityStats,
    seen: HashSet<u64>,
    sum_pos: [f64; 3],
    sum_sq: f64,
    /// Second pass over positions is impossible in a stream, so outliers
    /// are judged against the running centroid and RMS after a warm-up.
    checked_positions: u64,
}

impl QualityAccumulator {
    pub fn new(header: &Header) -> Self {
        let kind = header.flux_kind();
        let s = QualityStats {
            flux_header: header.total_flux(kind) as f64,
            flux_min: f32::MAX,
            flux_max: f32::MIN,
            min: [f32::MAX; 3],
            max: [f32::MIN; 3],
            ..Default::default()
        };
        Self {
            kind,
            header: header.clone(),
            s,
            seen: HashSet::new(),
            sum_pos: [0.0; 3],
            sum_sq: 0.0,
            checked_positions: 0,
        }
    }

    pub fn add(&mut self, r: &Ray) {
        let s = &mut self.s;
        s.rays += 1;
        // Flux.
        let flux = r.flux(self.kind);
        if flux.is_nan() {
            s.nan_flux += 1;
        } else {
            s.flux_sum += flux as f64;
            s.flux_min = s.flux_min.min(flux);
            s.flux_max = s.flux_max.max(flux);
            if flux == 0.0 {
                s.zero_flux += 1;
            } else if flux < 0.0 {
                s.negative_flux += 1;
            }
        }
        // Direction.
        let k = [r.kx, r.ky, r.kz];
        if k.iter().any(|v| !v.is_finite()) {
            s.nan_direction += 1;
        } else {
            let len = (k[0] * k[0] + k[1] * k[1] + k[2] * k[2]).sqrt();
            let err = (len - 1.0).abs();
            s.max_unit_error = s.max_unit_error.max(err);
            if err > UNIT_TOLERANCE {
                s.non_unit_direction += 1;
            }
            if r.kz < 0.0 {
                s.backward += 1;
            }
        }
        // Position.
        let p = [r.x, r.y, r.z];
        if p.iter().any(|v| !v.is_finite()) {
            s.nan_position += 1;
        } else {
            for (i, v) in p.iter().enumerate() {
                s.min[i] = s.min[i].min(*v);
                s.max[i] = s.max[i].max(*v);
            }
            let n = self.checked_positions as f64;
            if n >= 1000.0 {
                let c = [
                    self.sum_pos[0] / n,
                    self.sum_pos[1] / n,
                    self.sum_pos[2] / n,
                ];
                let rms = (self.sum_sq / n).sqrt().max(1e-9);
                let d = ((p[0] as f64 - c[0]).powi(2)
                    + (p[1] as f64 - c[1]).powi(2)
                    + (p[2] as f64 - c[2]).powi(2))
                .sqrt();
                if d > 4.0 * rms {
                    s.position_outliers += 1;
                }
            }
            self.checked_positions += 1;
            for (acc, v) in self.sum_pos.iter_mut().zip(p) {
                *acc += v as f64;
            }
            let n1 = self.checked_positions as f64;
            let c = [
                self.sum_pos[0] / n1,
                self.sum_pos[1] / n1,
                self.sum_pos[2] / n1,
            ];
            self.sum_sq += (p[0] as f64 - c[0]).powi(2)
                + (p[1] as f64 - c[1]).powi(2)
                + (p[2] as f64 - c[2]).powi(2);
        }
        // Duplicates (exact record).
        if (s.duplicate_check_rays as usize) < DUPLICATE_CHECK_LIMIT {
            s.duplicate_check_rays += 1;
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for v in [r.x, r.y, r.z, r.kx, r.ky, r.kz, flux] {
                h ^= v.to_bits() as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            if !self.seen.insert(h) {
                s.duplicates += 1;
            }
        }
        // Spectrum.
        if let Some(w) = r.wavelength_nm {
            if !w.is_finite() {
                s.nan_wavelength += 1;
            } else if let (Some(lo), Some(hi)) =
                (self.header.min_wavelength_nm, self.header.max_wavelength_nm)
            {
                if w < lo - 1e-3 || w > hi + 1e-3 {
                    s.wavelength_out_of_bounds += 1;
                }
            }
        }
        if let Some(i) = r.spectrum_index {
            if i == 0 || i as usize > self.header.spectra.len() {
                s.spectrum_index_out_of_range += 1;
            }
        }
    }

    pub fn finish(self) -> QualityReport {
        let h = &self.header;
        let mut s = self.s;
        let n = s.rays;
        if s.flux_min == f32::MAX {
            s.flux_min = 0.0;
            s.flux_max = 0.0;
        }
        if s.min[0] == f32::MAX {
            s.min = [0.0; 3];
            s.max = [0.0; 3];
        }
        let mean = if n > 0 { s.flux_sum / n as f64 } else { 0.0 };
        s.flux_outliers = 0; // recomputed below from max only (no second pass)
        let mut f = Vec::new();
        let pct = |c: u64| {
            if n > 0 {
                c as f64 / n as f64 * 100.0
            } else {
                0.0
            }
        };
        let unit = match self.kind {
            FluxKind::Photometric => "lm",
            _ => "W",
        };

        // Header-level.
        // A header built in code has n_rays 0: nothing to compare.
        if h.n_rays > 0 && n != h.n_rays {
            f.push(Finding {
                severity: Severity::Error,
                code: "ray-count",
                message: format!("header promises {} rays, the file holds {n}", h.n_rays),
            });
        }
        if s.flux_header == 0.0 && n > 0 {
            f.push(Finding {
                severity: Severity::Warning,
                code: "header-flux-unknown",
                message: format!(
                    "the header total flux is unknown or zero; the rays sum to {:.4} {unit}",
                    s.flux_sum
                ),
            });
        } else if s.flux_header > 0.0 {
            let dev = (s.flux_sum - s.flux_header) / s.flux_header;
            if dev.abs() > 0.01 {
                f.push(Finding {
                    severity: Severity::Error,
                    code: "flux-mismatch",
                    message: format!(
                        "ray fluxes sum to {:.4} {unit}, header says {:.4} {unit} ({:+.1} %)",
                        s.flux_sum,
                        s.flux_header,
                        dev * 100.0
                    ),
                });
            } else if dev.abs() > 1e-4 {
                f.push(Finding {
                    severity: Severity::Info,
                    code: "flux-rounding",
                    message: format!(
                        "ray fluxes sum to {:.6} {unit}, header {:.6} {unit} ({:+.3} %)",
                        s.flux_sum,
                        s.flux_header,
                        dev * 100.0
                    ),
                });
            }
        }
        // A header with both totals and a spectrum: the lm/W it implies
        // must match the spectrum's luminous efficacy.
        if h.luminous_flux_lm > 0.0 && h.radiant_flux_w > 0.0 {
            if let Some(ler) = h
                .shared_spectrum()
                .and_then(crate::photometry::luminous_efficacy_lm_per_w)
            {
                let claimed = h.luminous_flux_lm as f64 / h.radiant_flux_w as f64;
                let dev = claimed / ler - 1.0;
                if ler > 1.0 && dev.abs() > 0.05 {
                    f.push(Finding {
                        severity: Severity::Warning,
                        code: "lm-per-w",
                        message: format!(
                            "header totals give {claimed:.1} lm/W but the spectral table's luminous efficacy is {ler:.1} lm/W ({:+.1} %)",
                            dev * 100.0
                        ),
                    });
                }
            }
        }
        if h.creation_method != 0 && h.creation_method != 1 {
            f.push(Finding {
                severity: Severity::Info,
                code: "creation-method",
                message: format!(
                    "creation method {} is neither 0 (simulation) nor 1 (measurement)",
                    h.creation_method
                ),
            });
        }
        if h.date_time.trim().is_empty() {
            f.push(Finding {
                severity: Severity::Info,
                code: "no-date",
                message: "no creation date in the header".into(),
            });
        }
        if h.text.name().trim().is_empty() {
            f.push(Finding {
                severity: Severity::Info,
                code: "no-name",
                message: "the light source name is empty".into(),
            });
        }
        if h.spectral_id == SpectralId::None && h.spectra.is_empty() && !h.flags.wavelength {
            f.push(Finding {
                severity: Severity::Info,
                code: "no-spectrum",
                message: "no spectral information: radiometric values cannot be converted to photometric ones".into(),
            });
        }

        // Rays.
        if s.nan_flux + s.nan_direction + s.nan_position > 0 {
            f.push(Finding {
                severity: Severity::Error,
                code: "nan-records",
                message: format!(
                    "{} rays carry NaN or infinite values (flux {}, direction {}, position {})",
                    s.nan_flux + s.nan_direction + s.nan_position,
                    s.nan_flux,
                    s.nan_direction,
                    s.nan_position
                ),
            });
        }
        if s.negative_flux > 0 {
            f.push(Finding {
                severity: Severity::Error,
                code: "negative-flux",
                message: format!("{} rays have negative flux", s.negative_flux),
            });
        }
        if s.zero_flux > 0 {
            f.push(Finding {
                severity: Severity::Warning,
                code: "zero-flux",
                message: format!(
                    "{} rays ({:.2} %) carry no flux and only cost time",
                    s.zero_flux,
                    pct(s.zero_flux)
                ),
            });
        }
        if mean > 0.0 && (s.flux_max as f64) > 100.0 * mean {
            f.push(Finding {
                severity: Severity::Warning,
                code: "flux-outlier",
                message: format!(
                    "the brightest ray carries {:.0} × the mean flux ({:.3e} vs {:.3e} {unit}); a few rays dominate the statistics",
                    s.flux_max as f64 / mean,
                    s.flux_max,
                    mean
                ),
            });
        }
        if s.non_unit_direction > 0 {
            f.push(Finding {
                severity: if pct(s.non_unit_direction) > 1.0 { Severity::Error } else { Severity::Warning },
                code: "non-unit-direction",
                message: format!(
                    "{} rays ({:.2} %) have a direction that is not a unit vector (worst | |k| − 1 | = {:.2e})",
                    s.non_unit_direction,
                    pct(s.non_unit_direction),
                    s.max_unit_error
                ),
            });
        }
        if s.backward > 0 {
            f.push(Finding {
                severity: Severity::Info,
                code: "backward-rays",
                message: format!(
                    "{} rays ({:.2} %) point into the lower hemisphere (z < 0)",
                    s.backward,
                    pct(s.backward)
                ),
            });
        }
        if s.position_outliers > 0 {
            f.push(Finding {
                severity: Severity::Warning,
                code: "position-outliers",
                message: format!(
                    "{} start points lie more than 4 × the RMS distance from the centroid; check units and stray rays",
                    s.position_outliers
                ),
            });
        }
        let extent = (0..3)
            .map(|i| (s.max[i] - s.min[i]).abs())
            .fold(0.0f32, f32::max);
        if n > 0 && extent > 1000.0 {
            f.push(Finding {
                severity: Severity::Warning,
                code: "extent",
                message: format!("start points span {extent:.0} mm; TM-25 positions are millimetres, is the file in µm?"),
            });
        }
        if s.duplicates > 0 {
            let p = s.duplicates as f64 / s.duplicate_check_rays.max(1) as f64 * 100.0;
            f.push(Finding {
                severity: if p > 1.0 { Severity::Warning } else { Severity::Info },
                code: "duplicates",
                message: format!(
                    "{} identical records ({:.2} % of the first {} rays); the set is smaller than it looks",
                    s.duplicates, p, s.duplicate_check_rays
                ),
            });
        }
        if s.nan_wavelength + s.wavelength_out_of_bounds > 0 {
            f.push(Finding {
                severity: Severity::Warning,
                code: "wavelength",
                message: format!(
                    "{} rays have a wavelength outside the header bounds and {} a NaN",
                    s.wavelength_out_of_bounds, s.nan_wavelength
                ),
            });
        }
        if s.spectrum_index_out_of_range > 0 {
            f.push(Finding {
                severity: Severity::Error,
                code: "spectrum-index",
                message: format!(
                    "{} rays point at a spectral table that does not exist",
                    s.spectrum_index_out_of_range
                ),
            });
        }

        // Statistics: rays into a 10° × 5° bin of a Lambertian source are
        // n × ΔΩ cos γ / π with ΔΩ = Δφ (cos γ1 − cos γ2). The polar cap is
        // tiny, so the representative bin is the one at γ = 30°–35°.
        let d_omega = 10f64.to_radians() * (30f64.to_radians().cos() - 35f64.to_radians().cos());
        let per_bin = n as f64 * d_omega * 32.5f64.to_radians().cos() / std::f64::consts::PI;
        let noise = if per_bin > 0.0 {
            1.0 / per_bin.sqrt()
        } else {
            1.0
        };
        if n > 0 && noise > 0.1 {
            f.push(Finding {
                severity: Severity::Info,
                code: "few-rays",
                message: format!(
                    "{n} rays give about {:.0} rays per 10° × 5° bin at γ = 30°, {:.0} % Monte-Carlo noise; use a larger package for a far field",
                    per_bin,
                    noise * 100.0
                ),
            });
        }
        f.sort_by_key(|a| std::cmp::Reverse(a.severity));
        QualityReport {
            stats: s,
            findings: f,
            far_field_noise: noise,
        }
    }
}

/// Convenience: report for rays in memory.
pub fn quality_report<'a, I: IntoIterator<Item = &'a Ray>>(
    header: &Header,
    rays: I,
) -> QualityReport {
    let mut acc = QualityAccumulator::new(header);
    for r in rays {
        acc.add(r);
    }
    acc.finish()
}
