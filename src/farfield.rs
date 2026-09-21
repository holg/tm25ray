//! Far-field intensity from ray directions: bin into a C/γ grid, divide by
//! solid angle. Start positions are ignored, which is exactly what the
//! far-field approximation discards.

use crate::header::FluxKind;
use crate::ray::Ray;

/// Unit of the binned intensity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntensityUnit {
    /// W/sr (radiometric files).
    WattPerSteradian,
    /// cd (photometric files).
    Candela,
}

/// Accumulates ray flux into a C/γ grid.
#[derive(Clone, Debug)]
pub struct FarFieldBuilder {
    c_step: f64,
    g_step: f64,
    n_c: usize,
    n_g: usize,
    kind: FluxKind,
    flux: Vec<f64>,
    total: f64,
    rays: u64,
}

impl FarFieldBuilder {
    /// `c_step` must divide 360, `g_step` must divide 180 (both in degrees).
    pub fn new(c_step_deg: f64, g_step_deg: f64, kind: FluxKind) -> Self {
        let n_c = (360.0 / c_step_deg).round().max(1.0) as usize;
        let n_g = (180.0 / g_step_deg).round().max(1.0) as usize;
        Self {
            c_step: 360.0 / n_c as f64,
            g_step: 180.0 / n_g as f64,
            n_c,
            n_g,
            kind,
            flux: vec![0.0; n_c * n_g],
            total: 0.0,
            rays: 0,
        }
    }

    #[inline]
    pub fn add(&mut self, ray: &Ray) {
        let (c, g) = ray.c_gamma();
        let ci = ((c / self.c_step).floor() as usize) % self.n_c;
        let gi = ((g / self.g_step).floor() as usize).min(self.n_g - 1);
        let f = ray.flux(self.kind) as f64;
        self.flux[ci * self.n_g + gi] += f;
        self.total += f;
        self.rays += 1;
    }

    pub fn extend<'a, I: IntoIterator<Item = &'a Ray>>(&mut self, rays: I) {
        for r in rays {
            self.add(r);
        }
    }

    pub fn finish(self) -> FarField {
        let mut intensity = vec![0.0; self.n_c * self.n_g];
        let dphi = self.c_step.to_radians();
        for gi in 0..self.n_g {
            let g1 = (gi as f64 * self.g_step).to_radians();
            let g2 = ((gi + 1) as f64 * self.g_step).to_radians();
            let domega = dphi * (g1.cos() - g2.cos());
            for ci in 0..self.n_c {
                let i = ci * self.n_g + gi;
                intensity[i] = if domega > 0.0 {
                    self.flux[i] / domega
                } else {
                    0.0
                };
            }
        }
        FarField {
            c_step: self.c_step,
            g_step: self.g_step,
            n_c: self.n_c,
            n_g: self.n_g,
            unit: match self.kind {
                FluxKind::Photometric => IntensityUnit::Candela,
                _ => IntensityUnit::WattPerSteradian,
            },
            intensity,
            total_flux: self.total,
            ray_count: self.rays,
        }
    }
}

/// Binned far-field intensity. Row = C plane, column = γ bin.
#[derive(Clone, Debug, PartialEq)]
pub struct FarField {
    c_step: f64,
    g_step: f64,
    n_c: usize,
    n_g: usize,
    pub unit: IntensityUnit,
    /// `n_c × n_g`, W/sr or cd.
    pub intensity: Vec<f64>,
    /// Sum of the binned ray flux (W or lm).
    pub total_flux: f64,
    pub ray_count: u64,
}

impl FarField {
    /// Bin every ray of `rays`.
    pub fn from_rays<'a, I: IntoIterator<Item = &'a Ray>>(
        rays: I,
        c_step_deg: f64,
        g_step_deg: f64,
        kind: FluxKind,
    ) -> Self {
        let mut b = FarFieldBuilder::new(c_step_deg, g_step_deg, kind);
        b.extend(rays);
        b.finish()
    }

    pub fn c_count(&self) -> usize {
        self.n_c
    }

    pub fn g_count(&self) -> usize {
        self.n_g
    }

    pub fn c_step(&self) -> f64 {
        self.c_step
    }

    pub fn g_step(&self) -> f64 {
        self.g_step
    }

    /// Bin centres in degrees.
    pub fn c_angles(&self) -> Vec<f64> {
        (0..self.n_c)
            .map(|i| (i as f64 + 0.5) * self.c_step)
            .collect()
    }

    /// Bin centres in degrees.
    pub fn g_angles(&self) -> Vec<f64> {
        (0..self.n_g)
            .map(|i| (i as f64 + 0.5) * self.g_step)
            .collect()
    }

    #[inline]
    pub fn intensity(&self, ci: usize, gi: usize) -> f64 {
        self.intensity[ci * self.n_g + gi]
    }

    pub fn max_intensity(&self) -> f64 {
        self.intensity.iter().cloned().fold(0.0, f64::max)
    }

    /// Nearest-bin lookup at arbitrary angles (degrees).
    pub fn sample_nearest(&self, c_deg: f64, g_deg: f64) -> f64 {
        let c = c_deg.rem_euclid(360.0);
        let ci = ((c / self.c_step).floor() as usize) % self.n_c;
        let gi = ((g_deg.clamp(0.0, 180.0) / self.g_step).floor() as usize).min(self.n_g - 1);
        self.intensity(ci, gi)
    }

    /// Bilinear interpolation between bin centres (C wraps, γ clamps).
    /// Suitable for a `PhotometricData::sample` implementation; equals the
    /// bin value exactly at bin centres.
    pub fn sample(&self, c_deg: f64, g_deg: f64) -> f64 {
        let cf = (c_deg.rem_euclid(360.0) / self.c_step - 0.5).rem_euclid(self.n_c as f64);
        let c0 = cf.floor() as usize % self.n_c;
        let c1 = (c0 + 1) % self.n_c;
        let tc = cf - cf.floor();
        let gf = (g_deg.clamp(0.0, 180.0) / self.g_step - 0.5).clamp(0.0, (self.n_g - 1) as f64);
        let g0 = gf.floor() as usize;
        let g1 = (g0 + 1).min(self.n_g - 1);
        let tg = gf - g0 as f64;
        let a = self.intensity(c0, g0) * (1.0 - tc) + self.intensity(c1, g0) * tc;
        let b = self.intensity(c0, g1) * (1.0 - tc) + self.intensity(c1, g1) * tc;
        a * (1.0 - tg) + b * tg
    }

    /// Box-smoothed copy: every bin becomes the mean of its neighbours within
    /// `c_radius` bins in C (wrapping) and `g_radius` bins in γ (clamped).
    /// Bins whose centre lies within `pole_deg` of either pole are replaced
    /// by their azimuthal average, because their solid angle is so small
    /// that Monte Carlo noise dominates there. Total flux is unchanged up to
    /// the smoothing of the edge bins.
    pub fn smoothed(&self, c_radius: usize, g_radius: usize, pole_deg: f64) -> FarField {
        let mut out = self.clone();
        for gi in 0..self.n_g {
            let g_lo = gi.saturating_sub(g_radius);
            let g_hi = (gi + g_radius).min(self.n_g - 1);
            for ci in 0..self.n_c {
                let mut sum = 0.0;
                let mut n = 0.0;
                for dg in g_lo..=g_hi {
                    for dc in 0..=(2 * c_radius) {
                        let cc = (ci + self.n_c + dc - c_radius) % self.n_c;
                        sum += self.intensity(cc, dg);
                        n += 1.0;
                    }
                }
                out.intensity[ci * self.n_g + gi] = sum / n;
            }
        }
        let avg = out.azimuthal_average();
        for (gi, &mean) in avg.iter().enumerate() {
            let centre = (gi as f64 + 0.5) * self.g_step;
            if centre <= pole_deg || centre >= 180.0 - pole_deg {
                for ci in 0..self.n_c {
                    out.intensity[ci * self.n_g + gi] = mean;
                }
            }
        }
        out
    }

    /// Intensity averaged over all C planes, one value per γ bin.
    pub fn azimuthal_average(&self) -> Vec<f64> {
        (0..self.n_g)
            .map(|gi| (0..self.n_c).map(|ci| self.intensity(ci, gi)).sum::<f64>() / self.n_c as f64)
            .collect()
    }

    /// γ (degrees, interpolated between bin centres) where the azimuthally
    /// averaged intensity first drops below half its maximum.
    pub fn half_intensity_gamma(&self) -> Option<f64> {
        let prof = self.azimuthal_average();
        let max = prof.iter().cloned().fold(0.0, f64::max);
        if max <= 0.0 {
            return None;
        }
        let half = max / 2.0;
        let g = self.g_angles();
        for i in 1..prof.len() {
            if prof[i - 1] >= half && prof[i] < half {
                let t = (prof[i - 1] - half) / (prof[i - 1] - prof[i]);
                return Some(g[i - 1] + t * (g[i] - g[i - 1]));
            }
        }
        None
    }

    /// Fraction of the flux in the γ < 90° hemisphere (the emission
    /// hemisphere for an LED, "downward" in LDT terms).
    pub fn forward_fraction(&self) -> f64 {
        if self.total_flux <= 0.0 {
            return 0.0;
        }
        let dphi = self.c_step.to_radians();
        let mut fwd = 0.0;
        for gi in 0..self.n_g {
            let g1 = (gi as f64 * self.g_step).to_radians();
            let g2 = ((gi + 1) as f64 * self.g_step).to_radians();
            if (gi as f64 + 0.5) * self.g_step >= 90.0 {
                break;
            }
            let domega = dphi * (g1.cos() - g2.cos());
            for ci in 0..self.n_c {
                fwd += self.intensity(ci, gi) * domega;
            }
        }
        (fwd / self.total_flux).clamp(0.0, 1.0)
    }
}
