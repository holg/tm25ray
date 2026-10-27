//! Luminance (or radiance) map of a ray set seen from a direction: the
//! near-field image a camera or an eye would see, in cd/m² or W/(sr·m²) per
//! pixel. Method and noise figures: `docs/luminance.md` in the repository.
//!
//! Rays inside a cone of half-angle `α` around the view direction `v` are
//! selected (`k · v ≥ cos α`, solid angle `Ω = 2π (1 − cos α)`), their start
//! points are projected orthographically onto the plane perpendicular to
//! `v`, binned into square pixels of side `p`, and each pixel's flux is
//! divided by `p² · Ω`. The pixel lies on the plane perpendicular to the
//! view, so its area already is the projected area: there is no extra
//! cosine. A flat Lambertian emitter of area `A` and flux `Φ` must come out
//! as `Φ / (π A)` at every pixel and from every direction, which the
//! generated ground-truth file and the tests check.
//!
//! The estimate is an average over the pixel and over the cone, with
//! relative Monte-Carlo noise about `1 / √n` for `n` rays per pixel; the
//! map reports the mean rays per lit pixel so the user can judge it.

use crate::header::FluxKind;
use crate::ray::Ray;

/// A luminance map and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct LuminanceMap {
    /// Unit view direction, from the source towards the observer, TM-25 frame.
    pub direction: [f32; 3],
    /// Image axes in the TM-25 frame: a pixel at `(u, v)` is the start point
    /// `p` with `u = p · e1`, `v = p · e2`.
    pub e1: [f32; 3],
    pub e2: [f32; 3],
    pub cone_deg: f32,
    /// Pixel side actually used, mm (enlarged from the request when the
    /// image would exceed `max_side` pixels).
    pub pixel_mm: f32,
    pub width: usize,
    pub height: usize,
    /// `(u, v)` of the lower-left corner of pixel `(0, 0)`, mm.
    pub origin_mm: [f32; 2],
    /// Row-major, `height` rows of `width`, in cd/m² or W/(sr·m²).
    pub values: Vec<f64>,
    pub kind: FluxKind,
    /// Rays inside the cone.
    pub rays: usize,
    pub peak: f64,
    /// Mean over pixels that received at least one ray.
    pub mean_lit: f64,
    pub lit_pixels: usize,
    /// Rays per lit pixel: `1 / √` of it is the relative noise per pixel.
    pub rays_per_lit_pixel: f64,
}

impl LuminanceMap {
    pub fn unit(&self) -> &'static str {
        match self.kind {
            FluxKind::Photometric => "cd/m²",
            _ => "W/(sr·m²)",
        }
    }

    pub fn get(&self, x: usize, y: usize) -> f64 {
        self.values[y * self.width + x]
    }

    /// Solid angle of the acceptance cone, sr.
    pub fn solid_angle(&self) -> f64 {
        2.0 * std::f64::consts::PI * (1.0 - (self.cone_deg as f64).to_radians().cos())
    }
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !(n > 0.0) || !n.is_finite() {
        return None;
    }
    Some([v[0] / n, v[1] / n, v[2] / n])
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Image axes for a view direction, as a camera orbiting the emission axis
/// sees it: `e1` is horizontal (perpendicular to `v` and to the TM-25 z
/// axis), `e2 = v × e1` points "up" (towards +z projected). Looking straight
/// down the emission axis (`v = +z`) gives x to the right and y up.
pub fn image_axes(v: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let e1 = if v[2].abs() < 0.9 {
        normalize(cross([0.0, 0.0, 1.0], v)).unwrap_or([1.0, 0.0, 0.0])
    } else {
        normalize(cross([0.0, 1.0, 0.0], v)).unwrap_or([1.0, 0.0, 0.0])
    };
    let e2 = normalize(cross(v, e1)).unwrap_or([0.0, 1.0, 0.0]);
    (e1, e2)
}

/// Compute the map. `max_side` caps the image size; the pixel grows to fit.
/// `None` when the direction is degenerate or no ray falls into the cone.
pub fn luminance_map<'a, I: IntoIterator<Item = &'a Ray>>(
    rays: I,
    kind: FluxKind,
    direction: [f32; 3],
    cone_deg: f32,
    pixel_mm: f32,
    max_side: usize,
) -> Option<LuminanceMap> {
    let v = normalize(direction)?;
    let cone = cone_deg.clamp(0.05, 89.0);
    let cos_cone = cone.to_radians().cos();
    let (e1, e2) = image_axes(v);
    // Pass 1: select and project.
    let mut sel: Vec<(f32, f32, f32)> = Vec::new();
    let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for r in rays {
        if r.kx * v[0] + r.ky * v[1] + r.kz * v[2] < cos_cone {
            continue;
        }
        let p = [r.x, r.y, r.z];
        if p.iter().any(|c| !c.is_finite()) {
            continue;
        }
        let u = p[0] * e1[0] + p[1] * e1[1] + p[2] * e1[2];
        let w = p[0] * e2[0] + p[1] * e2[1] + p[2] * e2[2];
        umin = umin.min(u);
        umax = umax.max(u);
        vmin = vmin.min(w);
        vmax = vmax.max(w);
        sel.push((u, w, r.flux(kind)));
    }
    if sel.is_empty() {
        return None;
    }
    let max_side = max_side.max(1);
    let mut pixel = pixel_mm.max(1e-4);
    let extent = (umax - umin).max(vmax - vmin).max(pixel);
    if extent / pixel > max_side as f32 {
        pixel = extent / max_side as f32;
    }
    let width = (((umax - umin) / pixel).floor() as usize + 1).min(max_side);
    let height = (((vmax - vmin) / pixel).floor() as usize + 1).min(max_side);
    let origin = [umin, vmin];
    let mut values = vec![0.0f64; width * height];
    let mut counts = vec![0u32; width * height];
    for (u, w, flux) in sel.iter() {
        let x = (((u - origin[0]) / pixel) as usize).min(width - 1);
        let y = (((w - origin[1]) / pixel) as usize).min(height - 1);
        values[y * width + x] += *flux as f64;
        counts[y * width + x] += 1;
    }
    let omega = 2.0 * std::f64::consts::PI * (1.0 - (cone as f64).to_radians().cos());
    let area_m2 = (pixel as f64 * 1e-3).powi(2);
    let scale = 1.0 / (area_m2 * omega);
    let mut peak = 0.0f64;
    let mut sum_lit = 0.0f64;
    let mut lit = 0usize;
    for (val, c) in values.iter_mut().zip(&counts) {
        *val *= scale;
        if *c > 0 {
            lit += 1;
            sum_lit += *val;
            peak = peak.max(*val);
        }
    }
    Some(LuminanceMap {
        direction: v,
        e1,
        e2,
        cone_deg: cone,
        pixel_mm: pixel,
        width,
        height,
        origin_mm: origin,
        values,
        kind,
        rays: sel.len(),
        peak,
        mean_lit: if lit > 0 { sum_lit / lit as f64 } else { 0.0 },
        lit_pixels: lit,
        rays_per_lit_pixel: if lit > 0 {
            sel.len() as f64 / lit as f64
        } else {
            0.0
        },
    })
}

/// Luminance of a flat Lambertian emitter: `Φ / (π A)`, with `area_mm2` in
/// mm² and `flux` in the file's unit. The reference value for a ground
/// truth file.
pub fn lambertian_luminance(flux: f64, area_mm2: f64) -> f64 {
    flux / (std::f64::consts::PI * area_mm2 * 1e-6)
}
