//! Flux-preserving subsampling: random pick from an in-memory view, or
//! reservoir sampling over a stream. Deterministic for a given seed and
//! free of external RNG dependencies (SplitMix64 + xorshift*).

use crate::ray::{Ray, RayView};

/// Small deterministic PRNG (xorshift64*, seeded through SplitMix64).
#[derive(Clone, Debug)]
pub struct Rng64(u64);

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        // SplitMix64 to spread poor seeds (e.g. 0, 1) over the state space.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self(if z == 0 { 0x2545_F491_4F6C_DD1D } else { z })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        // Rejection to avoid modulo bias.
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }
}

/// Multiply both fluxes of every ray by `factor`.
fn rescale(rays: &mut [Ray], factor_rad: f64, factor_lum: f64) {
    for r in rays.iter_mut() {
        if let Some(v) = r.radiant_flux_w.as_mut() {
            *v = (*v as f64 * factor_rad) as f32;
        }
        if let Some(v) = r.luminous_flux_lm.as_mut() {
            *v = (*v as f64 * factor_lum) as f32;
        }
    }
}

fn factor(total: f64, kept: f64) -> f64 {
    if kept > 0.0 && total > 0.0 {
        total / kept
    } else {
        1.0
    }
}

/// Pick `n` distinct rays uniformly at random (Floyd's algorithm) and
/// rescale their fluxes so the kept set sums to the view's totals. Returns
/// all rays (unscaled) when `n >= len`.
pub fn subsample(view: &RayView<'_>, n: usize, seed: u64) -> Vec<Ray> {
    let len = view.len();
    if n >= len {
        return view.iter().collect();
    }
    let (mut tot_rad, mut tot_lum) = (0.0f64, 0.0f64);
    for r in view.iter() {
        tot_rad += r.radiant_flux_w.unwrap_or(0.0) as f64;
        tot_lum += r.luminous_flux_lm.unwrap_or(0.0) as f64;
    }
    let mut rng = Rng64::new(seed);
    // Floyd: for j in len-n..len, pick t in [0, j]; insert t unless present, else j.
    let mut chosen = std::collections::HashSet::with_capacity(n);
    for j in (len - n)..len {
        let t = rng.below(j as u64 + 1) as usize;
        if !chosen.insert(t) {
            chosen.insert(j);
        }
    }
    let mut idx: Vec<usize> = chosen.into_iter().collect();
    idx.sort_unstable();
    let mut out: Vec<Ray> = idx.iter().filter_map(|&i| view.get(i)).collect();
    let (mut k_rad, mut k_lum) = (0.0f64, 0.0f64);
    for r in &out {
        k_rad += r.radiant_flux_w.unwrap_or(0.0) as f64;
        k_lum += r.luminous_flux_lm.unwrap_or(0.0) as f64;
    }
    rescale(&mut out, factor(tot_rad, k_rad), factor(tot_lum, k_lum));
    out
}

/// Reservoir sampler (Algorithm R) for streams: keeps `capacity` rays,
/// tracks the running flux totals, and rescales on `finish`.
#[derive(Clone, Debug)]
pub struct Reservoir {
    capacity: usize,
    rng: Rng64,
    kept: Vec<Ray>,
    seen: u64,
    total_rad: f64,
    total_lum: f64,
}

impl Reservoir {
    pub fn new(capacity: usize, seed: u64) -> Self {
        Self {
            capacity,
            rng: Rng64::new(seed),
            kept: Vec::with_capacity(capacity.min(1 << 20)),
            seen: 0,
            total_rad: 0.0,
            total_lum: 0.0,
        }
    }

    pub fn push(&mut self, ray: Ray) {
        self.total_rad += ray.radiant_flux_w.unwrap_or(0.0) as f64;
        self.total_lum += ray.luminous_flux_lm.unwrap_or(0.0) as f64;
        self.seen += 1;
        if self.kept.len() < self.capacity {
            self.kept.push(ray);
        } else {
            let j = self.rng.below(self.seen) as usize;
            if j < self.capacity {
                self.kept[j] = ray;
            }
        }
    }

    pub fn extend<I: IntoIterator<Item = Ray>>(&mut self, rays: I) {
        for r in rays {
            self.push(r);
        }
    }

    /// Rays seen so far.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// Running totals `(radiant W, luminous lm)` over everything pushed.
    pub fn totals(&self) -> (f64, f64) {
        (self.total_rad, self.total_lum)
    }

    /// The kept rays with fluxes rescaled to the stream totals.
    pub fn finish(self) -> Vec<Ray> {
        let mut out = self.kept;
        let (mut k_rad, mut k_lum) = (0.0f64, 0.0f64);
        for r in &out {
            k_rad += r.radiant_flux_w.unwrap_or(0.0) as f64;
            k_lum += r.luminous_flux_lm.unwrap_or(0.0) as f64;
        }
        rescale(
            &mut out,
            factor(self.total_rad, k_rad),
            factor(self.total_lum, k_lum),
        );
        out
    }
}
