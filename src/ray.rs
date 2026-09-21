//! Ray records: decoding, a zero-copy view over the ray block, and the
//! coordinate helpers shared with the viewer.

use crate::header::{Column, FluxKind, Header};

/// One decoded ray. Positions in mm, direction a unit vector, fluxes in
/// W (radiant) or lm (luminous). Columns the file does not carry are `None`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Ray {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub kx: f32,
    pub ky: f32,
    pub kz: f32,
    pub radiant_flux_w: Option<f32>,
    pub luminous_flux_lm: Option<f32>,
    pub wavelength_nm: Option<f32>,
    /// 1-based index into `Header::spectra` (spectral id 4).
    pub spectrum_index: Option<u32>,
}

impl Ray {
    /// Position + direction only.
    pub fn new(pos: [f32; 3], dir: [f32; 3]) -> Self {
        Self {
            x: pos[0],
            y: pos[1],
            z: pos[2],
            kx: dir[0],
            ky: dir[1],
            kz: dir[2],
            ..Default::default()
        }
    }

    pub fn with_radiant_flux(mut self, w: f32) -> Self {
        self.radiant_flux_w = Some(w);
        self
    }

    pub fn with_luminous_flux(mut self, lm: f32) -> Self {
        self.luminous_flux_lm = Some(lm);
        self
    }

    pub fn position(&self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn direction(&self) -> [f32; 3] {
        [self.kx, self.ky, self.kz]
    }

    /// The flux for `kind`; `Both` prefers radiant. 0 when absent.
    pub fn flux(&self, kind: FluxKind) -> f32 {
        match kind {
            FluxKind::Radiometric | FluxKind::Both => self.radiant_flux_w.unwrap_or(0.0),
            FluxKind::Photometric => self.luminous_flux_lm.unwrap_or(0.0),
        }
    }

    /// |k|, should be 1.
    pub fn direction_norm(&self) -> f32 {
        (self.kx * self.kx + self.ky * self.ky + self.kz * self.kz).sqrt()
    }

    /// `(C, γ)` in degrees, see [`direction_to_c_gamma`].
    pub fn c_gamma(&self) -> (f64, f64) {
        direction_to_c_gamma(self.kx as f64, self.ky as f64, self.kz as f64)
    }

    /// Position in Bevy space, see [`to_bevy`].
    pub fn bevy_position(&self) -> [f32; 3] {
        to_bevy(self.position())
    }

    /// Direction in Bevy space, see [`to_bevy`].
    pub fn bevy_direction(&self) -> [f32; 3] {
        to_bevy(self.direction())
    }
}

/// Map a TM-25 direction to EULUMDAT `(C, γ)` in degrees.
///
/// γ is the polar angle from the TM-25 +z axis (the emission axis), so
/// γ = 0 is the LDT nadir and 180 the zenith. C is the azimuth measured from
/// +x towards +y, in `[0, 360)`.
pub fn direction_to_c_gamma(kx: f64, ky: f64, kz: f64) -> (f64, f64) {
    let n = (kx * kx + ky * ky + kz * kz).sqrt();
    if n <= 0.0 {
        return (0.0, 0.0);
    }
    let gamma = (kz / n).clamp(-1.0, 1.0).acos().to_degrees();
    let mut c = ky.atan2(kx).to_degrees();
    if c < 0.0 {
        c += 360.0;
    }
    if c >= 360.0 {
        c -= 360.0;
    }
    (c, gamma)
}

/// Map a TM-25 vector `(x, y, z)` to Bevy `(x, −z, y)`.
///
/// This matches the photometric solid of the eulumdat-rs Bevy viewer, which
/// puts γ = 0 (the emission axis) on Bevy −Y, C = 0 on +X and C = 90 on +Z,
/// so a ray-derived intensity solid overlays an LDT solid with no extra
/// rotation. It is a
/// proper rotation (−90° about X), so handedness is preserved.
pub fn to_bevy(v: [f32; 3]) -> [f32; 3] {
    [v[0], -v[2], v[1]]
}

/// Inverse of [`to_bevy`].
pub fn from_bevy(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

/// Byte layout of one ray record.
#[derive(Clone, Debug, PartialEq)]
pub struct RayLayout {
    pub columns: Vec<Column>,
    pub record_size: usize,
    radiant: Option<usize>,
    luminous: Option<usize>,
    wavelength: Option<usize>,
    spectrum_index: Option<usize>,
}

impl RayLayout {
    pub fn from_header(h: &Header) -> Self {
        Self::from_columns(h.columns.clone())
    }

    pub fn from_columns(columns: Vec<Column>) -> Self {
        let find = |c: Column| columns.iter().position(|x| *x == c);
        Self {
            radiant: find(Column::RadiantFlux),
            luminous: find(Column::LuminousFlux),
            wavelength: find(Column::Wavelength),
            spectrum_index: find(Column::SpectrumIndex),
            record_size: columns.len() * 4,
            columns,
        }
    }

    #[inline]
    fn f32_at(rec: &[u8], i: usize) -> f32 {
        f32::from_le_bytes([rec[4 * i], rec[4 * i + 1], rec[4 * i + 2], rec[4 * i + 3]])
    }

    /// Decode one record. `rec` must be at least `record_size` bytes.
    pub fn decode(&self, rec: &[u8]) -> Ray {
        debug_assert!(rec.len() >= self.record_size);
        Ray {
            x: Self::f32_at(rec, 0),
            y: Self::f32_at(rec, 1),
            z: Self::f32_at(rec, 2),
            kx: Self::f32_at(rec, 3),
            ky: Self::f32_at(rec, 4),
            kz: Self::f32_at(rec, 5),
            radiant_flux_w: self.radiant.map(|i| Self::f32_at(rec, i)),
            luminous_flux_lm: self.luminous.map(|i| Self::f32_at(rec, i)),
            wavelength_nm: self.wavelength.map(|i| Self::f32_at(rec, i)),
            spectrum_index: self
                .spectrum_index
                .map(|i| Self::f32_at(rec, i).round().max(0.0) as u32),
        }
    }

    /// Any column of a raw record (Stokes, tristimulus, additional items included).
    pub fn field(&self, rec: &[u8], col: Column) -> Option<f32> {
        self.columns
            .iter()
            .position(|c| *c == col)
            .map(|i| Self::f32_at(rec, i))
    }

    /// Append the record for `ray`. Columns the `Ray` struct does not carry
    /// (Stokes, tristimulus, additional items) are written as 0.
    pub fn encode(&self, ray: &Ray, out: &mut Vec<u8>) {
        for col in &self.columns {
            let v = match col {
                Column::X => ray.x,
                Column::Y => ray.y,
                Column::Z => ray.z,
                Column::Kx => ray.kx,
                Column::Ky => ray.ky,
                Column::Kz => ray.kz,
                Column::RadiantFlux => ray.radiant_flux_w.unwrap_or(0.0),
                Column::LuminousFlux => ray.luminous_flux_lm.unwrap_or(0.0),
                Column::Wavelength => ray.wavelength_nm.unwrap_or(0.0),
                Column::SpectrumIndex => ray.spectrum_index.unwrap_or(0) as f32,
                _ => 0.0,
            };
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// Zero-copy view over a ray block.
#[derive(Clone, Debug)]
pub struct RayView<'a> {
    layout: RayLayout,
    data: &'a [u8],
    len: usize,
}

impl<'a> RayView<'a> {
    /// `data` must hold exactly `len` records (callers check the size).
    pub fn new(layout: RayLayout, data: &'a [u8], len: usize) -> Self {
        debug_assert!(data.len() >= len * layout.record_size);
        Self { layout, data, len }
    }

    pub fn layout(&self) -> &RayLayout {
        &self.layout
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Raw bytes of record `i`.
    pub fn raw(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.len {
            return None;
        }
        let s = i * self.layout.record_size;
        Some(&self.data[s..s + self.layout.record_size])
    }

    pub fn get(&self, i: usize) -> Option<Ray> {
        self.raw(i).map(|r| self.layout.decode(r))
    }

    pub fn iter(&self) -> RayIter<'a> {
        RayIter {
            layout: self.layout.clone(),
            data: self.data,
            i: 0,
            len: self.len,
        }
    }

    /// The whole block as bytes (for uploading to a GPU buffer as-is).
    pub fn bytes(&self) -> &'a [u8] {
        &self.data[..self.len * self.layout.record_size]
    }
}

impl<'a> IntoIterator for &RayView<'a> {
    type Item = Ray;
    type IntoIter = RayIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct RayIter<'a> {
    layout: RayLayout,
    data: &'a [u8],
    i: usize,
    len: usize,
}

impl Iterator for RayIter<'_> {
    type Item = Ray;
    fn next(&mut self) -> Option<Ray> {
        if self.i >= self.len {
            return None;
        }
        let s = self.i * self.layout.record_size;
        self.i += 1;
        Some(
            self.layout
                .decode(&self.data[s..s + self.layout.record_size]),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len - self.i;
        (n, Some(n))
    }
}

impl ExactSizeIterator for RayIter<'_> {}
