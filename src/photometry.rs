//! Photopic luminous efficiency V(λ) and what follows from it: the
//! luminous efficacy of a spectral table, which converts a radiometric
//! file's watts into lumens and checks a vendor's lm/W against its own
//! spectrum.
//!
//! The table is the CIE 1924 photopic V(λ) (identical to the CIE 1931 ȳ)
//! at 5 nm from 380 to 780 nm, taken from `eulumdat-spectrum`; values are
//! interpolated linearly and are 0 outside the range.

use crate::header::SpectralTable;
use crate::mix::table_value_at;

/// Maximum luminous efficacy, lm/W at 555 nm.
pub const KM: f64 = 683.0;
const V_START_NM: f64 = 380.0;
const V_STEP_NM: f64 = 5.0;
#[rustfmt::skip]
const V_LAMBDA: [f64; 81] = [
    0.000039, 0.000064, 0.000120, 0.000217, 0.000396, 0.000640, 0.001210, 0.002180, 0.004000, 0.007300,
    0.011600, 0.016840, 0.023000, 0.029800, 0.038000, 0.048000, 0.060000, 0.073900, 0.090980, 0.112600,
    0.139020, 0.169300, 0.208020, 0.258600, 0.323000, 0.407300, 0.503000, 0.608200, 0.710000, 0.793200,
    0.862000, 0.914850, 0.954000, 0.980300, 0.994950, 1.000000, 0.995000, 0.978600, 0.952000, 0.915400,
    0.870000, 0.816300, 0.757000, 0.694900, 0.631000, 0.566800, 0.503000, 0.441200, 0.381000, 0.321000,
    0.265000, 0.217000, 0.175000, 0.138200, 0.107000, 0.081600, 0.061000, 0.044580, 0.032000, 0.023200,
    0.017000, 0.011920, 0.008210, 0.005723, 0.004102, 0.002929, 0.002091, 0.001484, 0.001047, 0.000740,
    0.000520, 0.000361, 0.000249, 0.000172, 0.000120, 0.000085, 0.000060, 0.000042, 0.000030, 0.000021,
    0.000015,
];

/// V(λ) at `nm`, linearly interpolated, 0 outside 380–780 nm.
pub fn v_lambda(nm: f64) -> f64 {
    let x = (nm - V_START_NM) / V_STEP_NM;
    if !(x.is_finite() && x >= 0.0) || x > (V_LAMBDA.len() - 1) as f64 {
        return 0.0;
    }
    let i = (x.floor() as usize).min(V_LAMBDA.len() - 2);
    let f = x - i as f64;
    V_LAMBDA[i] + (V_LAMBDA[i + 1] - V_LAMBDA[i]) * f
}

/// Luminous efficacy of radiation of a spectral table, lm/W:
/// `683 ∫ V(λ) S(λ) dλ / ∫ S(λ) dλ`, integrated at 1 nm over the table's
/// range. `None` for a table with no energy.
pub fn luminous_efficacy_lm_per_w(t: &SpectralTable) -> Option<f64> {
    if t.len() < 2 {
        return None;
    }
    let (lo, hi) = (
        t.wavelengths_nm[0] as f64,
        t.wavelengths_nm[t.len() - 1] as f64,
    );
    let (mut num, mut den) = (0.0, 0.0);
    let mut nm = lo;
    while nm <= hi + 1e-9 {
        let s = table_value_at(t, nm as f32) as f64;
        num += v_lambda(nm) * s;
        den += s;
        nm += 1.0;
    }
    if den <= 0.0 {
        return None;
    }
    Some(KM * num / den)
}
