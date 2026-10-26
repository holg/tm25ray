//! LightTools binary ray files (`.ray`): the import format of Synopsys
//! LightTools, written here from a TM-25 ray set so a file inspected or
//! subsampled with this crate can go straight into an illumination design.
//!
//! The layout follows the LightTools documentation (checked against the
//! vendor's own description of the binary format in October 2026) as
//! implemented in the C++ reference `TM25RaySetTools`
//! (`LightToolsBinary.cpp`, Unlicense), and the TM-25 mapping follows its
//! `TM25ToLightToolsBinary`. A file written here was loaded by LightTools
//! with the expected power, ray count and bounding box:
//!
//! | Offset | Content |
//! |---|---|
//! | 0 | `LTRF` |
//! | 4 | u32 major version 2 (3 = with polarisation, not written here), u32 minor 0 |
//! | 12 | u32 data type: 0 radiometric (W), 1 photometric (lm) |
//! | 16 | u32 far field: 0 = records carry a start point (always here) |
//! | 20 | u32 colour info: 0 none, 2 = a wavelength per ray (1 = tristimulus, not written) |
//! | 24 | u32 length unit: 1 = mm (TM-25 positions are mm) |
//! | 28 | 3 × f32 origin, f32 total flux in W or lm |
//! | 44 | records of 7 f32 (`x y z kx ky kz flux`), 8 with the wavelength in nm |
//! | end | the 7 ASCII bytes `LTRFEND` |
//!
//! Everything is little-endian. LightTools treats the per-ray value as a
//! *relative* flux and scales the set to the header total, so the absolute
//! fluxes written here (which sum to that total) are consistent either way.
//! Not written or read: the tristimulus colour mode (colour info 1, 36-byte
//! records with CIE X Y Z in place of the flux), far-field files without
//! start points, and version 3 with Stokes parameters.
//!
//! **What a TM-25 → LightTools → TM-25 round trip keeps and loses.** Kept:
//! start point, direction and flux of every ray, the total flux, and a
//! wavelength per ray when the source has one (identifier 2). Lost, because
//! the LightTools record has no place for them: a shared spectral table
//! (identifier 3), several tables with a per-ray index (identifier 4), the
//! tristimulus and Stokes columns, user-defined columns, the wavelength
//! bounds, the creation date and the nine description strings. Geometry,
//! far field and irradiance survive unchanged; the spectrum does not.
//!
//! A radiometric TM-25 file becomes a
//! radiometric LightTools file; a photometric-only file a photometric one.
//! The wavelength column is written when the TM-25 file carries one per ray
//! (spectral identifier 2), which LightTools only accepts with radiometric
//! flux, as in TM-25. A shared spectral table (identifier 3) has no
//! counterpart in the record and is not written; export the `.spd` for it.

use std::io::Write;

use crate::error::{Result, Tm25Error};
use crate::header::{
    columns_from_flags, FluxKind, Header, KnownDataFlags, SpectralId, SpectralTable,
};
use crate::ray::Ray;

/// Magic at offset 0.
pub const LT_MAGIC: [u8; 4] = *b"LTRF";
/// Trailer after the last record.
pub const LT_END: [u8; 7] = *b"LTRFEND";
/// Header size for version 2 files.
pub const LT_HEADER_SIZE: usize = 44;
/// `Header::version` of a TM-25 header synthesised from a LightTools file by
/// [`LightToolsHeader::to_tm25_header`]: the LightTools major version, so a
/// reader pipeline built around [`Header`] can tell the two apart
/// ([`Header::is_lighttools`]). The TM-25 writer always writes 2013.
pub const LIGHTTOOLS_VERSION_TAG: i32 = 2;

/// True when `bytes` start with the LightTools magic.
pub fn is_lighttools(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == LT_MAGIC
}

/// Header of a LightTools binary ray file, version 2.
#[derive(Clone, Debug, PartialEq)]
pub struct LightToolsHeader {
    pub major_version: u32,
    pub minor_version: u32,
    /// `FluxKind::Radiometric` or `FluxKind::Photometric`.
    pub flux_kind: FluxKind,
    /// True when the records have no start point (never written by this crate).
    pub far_field: bool,
    /// Each record ends with a wavelength in nm.
    pub per_ray_wavelength: bool,
    /// 0 system, 1 mm, 2 cm, 3 m, 4 inch, 5 feet, 6 µm, 7 nm.
    pub length_unit: u32,
    pub origin: [f32; 3],
    /// Total flux, W or lm according to `flux_kind`.
    pub total_flux: f32,
}

impl LightToolsHeader {
    /// The header a TM-25 file maps to, as the reference implementation does
    /// it: radiant flux wins when both are present, a wavelength per ray is
    /// kept, positions stay in mm.
    pub fn from_tm25(h: &Header) -> Result<Self> {
        let (flux_kind, total_flux) = if h.flags.radiant_flux {
            (FluxKind::Radiometric, h.radiant_flux_w)
        } else if h.flags.luminous_flux {
            (FluxKind::Photometric, h.luminous_flux_lm)
        } else {
            return Err(Tm25Error::InvalidHeader(
                "no flux column to export to LightTools".into(),
            ));
        };
        let per_ray_wavelength =
            h.flags.wavelength && h.spectral_id == SpectralId::PerRayWavelength;
        if per_ray_wavelength && flux_kind == FluxKind::Photometric {
            return Err(Tm25Error::InvalidHeader(
                "LightTools does not combine a wavelength per ray with photometric flux".into(),
            ));
        }
        Ok(Self {
            major_version: 2,
            minor_version: 0,
            flux_kind,
            far_field: false,
            per_ray_wavelength,
            length_unit: 1,
            origin: [0.0; 3],
            total_flux,
        })
    }

    /// Rays in a file of `file_size` bytes: everything between the header
    /// and the `LTRFEND` trailer, in whole records.
    pub fn ray_count_for_size(&self, file_size: u64) -> u64 {
        file_size.saturating_sub((LT_HEADER_SIZE + LT_END.len()) as u64) / self.record_size() as u64
    }

    /// Size of a file holding `n_rays` records.
    pub fn file_size_for(&self, n_rays: u64) -> u64 {
        LT_HEADER_SIZE as u64 + n_rays * self.record_size() as u64 + LT_END.len() as u64
    }

    /// A TM-25 [`Header`] describing this file's records, so the same code
    /// that streams TM-25 ray blocks can stream a LightTools file: the record
    /// is `x y z kx ky kz flux [λ]`, exactly the TM-25 column order for a
    /// file with one flux column and an optional per-ray wavelength, so
    /// `columns`, `record_size` and `ray_start` (44) line up. `n_rays` comes
    /// from `file_size` (the trailer is excluded), 0 when unknown.
    /// `version` is [`LIGHTTOOLS_VERSION_TAG`]; the wavelength bounds are
    /// unknown (`None`), the spectral identifier is 2 with a wavelength
    /// column and 0 without.
    pub fn to_tm25_header(&self, name: &str, file_size: Option<u64>) -> Header {
        let mut flags = KnownDataFlags::radiometric();
        if self.flux_kind == FluxKind::Photometric {
            flags.radiant_flux = false;
            flags.luminous_flux = true;
        }
        flags.wavelength = self.per_ray_wavelength;
        let mut h = Header::new(flags);
        h.version = LIGHTTOOLS_VERSION_TAG;
        h.creation_method = -1;
        match self.flux_kind {
            FluxKind::Photometric => h.luminous_flux_lm = self.total_flux,
            _ => h.radiant_flux_w = self.total_flux,
        }
        h.spectral_id = if self.per_ray_wavelength {
            SpectralId::PerRayWavelength
        } else {
            SpectralId::None
        };
        h.text.raw[0] = name.to_string();
        h.text.raw[3] = "LightTools binary ray file".to_string();
        h.text.raw[8] = format!(
            "LightTools ray file v{}.{}, length unit {}, origin ({}, {}, {})",
            self.major_version,
            self.minor_version,
            match self.length_unit {
                0 => "system",
                1 => "mm",
                2 => "cm",
                3 => "m",
                4 => "inch",
                5 => "ft",
                6 => "µm",
                7 => "nm",
                _ => "?",
            },
            self.origin[0],
            self.origin[1],
            self.origin[2]
        );
        h.columns = columns_from_flags(&flags, 0);
        h.ray_start = LT_HEADER_SIZE;
        h.n_rays = file_size.map(|s| self.ray_count_for_size(s)).unwrap_or(0);
        h
    }

    /// Bytes per record: 28, or 32 with a wavelength.
    pub fn record_size(&self) -> usize {
        if self.per_ray_wavelength {
            32
        } else {
            28
        }
    }

    pub fn encode(&self) -> [u8; LT_HEADER_SIZE] {
        let mut out = [0u8; LT_HEADER_SIZE];
        let mut p = 0;
        let mut put = |b: [u8; 4]| {
            out[p..p + 4].copy_from_slice(&b);
            p += 4;
        };
        put(LT_MAGIC);
        put(self.major_version.to_le_bytes());
        put(self.minor_version.to_le_bytes());
        put(((self.flux_kind == FluxKind::Photometric) as u32).to_le_bytes());
        put((self.far_field as u32).to_le_bytes());
        put((if self.per_ray_wavelength { 2u32 } else { 0 }).to_le_bytes());
        put(self.length_unit.to_le_bytes());
        put(self.origin[0].to_le_bytes());
        put(self.origin[1].to_le_bytes());
        put(self.origin[2].to_le_bytes());
        put(self.total_flux.to_le_bytes());
        out
    }

    /// Parse the header at the start of `bytes`. Version 3 (polarisation)
    /// and the tristimulus colour mode are reported as unsupported, like the
    /// reference does.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < LT_HEADER_SIZE {
            return Err(Tm25Error::Truncated {
                needed: LT_HEADER_SIZE,
                have: bytes.len(),
            });
        }
        let u32_at =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let f32_at = |i: usize| f32::from_bits(u32_at(i));
        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if magic != LT_MAGIC {
            return Err(Tm25Error::BadMagic(magic));
        }
        let major_version = u32_at(4);
        if major_version != 2 {
            return Err(Tm25Error::UnsupportedVersion(major_version as i32));
        }
        let flux_kind = match u32_at(12) {
            0 => FluxKind::Radiometric,
            1 => FluxKind::Photometric,
            v => {
                return Err(Tm25Error::InvalidHeader(format!(
                    "LightTools data type {v}"
                )))
            }
        };
        let per_ray_wavelength = match u32_at(20) {
            0 => false,
            2 => true,
            v => {
                return Err(Tm25Error::InvalidHeader(format!(
                    "LightTools colour info {v}"
                )))
            }
        };
        Ok(Self {
            major_version,
            minor_version: u32_at(8),
            flux_kind,
            far_field: u32_at(16) != 0,
            per_ray_wavelength,
            length_unit: u32_at(24),
            origin: [f32_at(28), f32_at(32), f32_at(36)],
            total_flux: f32_at(40),
        })
    }
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append one record for `ray`. The flux column is the one `lt.flux_kind`
/// names; a missing wavelength is written as 0.
fn encode_ray(lt: &LightToolsHeader, ray: &Ray, out: &mut Vec<u8>) {
    put_f32(out, ray.x);
    put_f32(out, ray.y);
    put_f32(out, ray.z);
    put_f32(out, ray.kx);
    put_f32(out, ray.ky);
    put_f32(out, ray.kz);
    let flux = match lt.flux_kind {
        FluxKind::Photometric => ray.luminous_flux_lm.unwrap_or(0.0),
        _ => ray.radiant_flux_w.unwrap_or(0.0),
    };
    put_f32(out, flux);
    if lt.per_ray_wavelength {
        put_f32(out, ray.wavelength_nm.unwrap_or(0.0));
    }
}

/// Write a LightTools binary ray file for a TM-25 ray set. `rays` is any
/// iterator of decoded rays, so a `RayView`, a subsample or a stream all
/// work; `header.total_flux` is written as the file total, which should be
/// the sum of the ray fluxes (a flux-rescaled subsample keeps it so).
pub fn write_lighttools<W: Write, I: IntoIterator<Item = Ray>>(
    mut w: W,
    header: &LightToolsHeader,
    rays: I,
) -> Result<u64> {
    w.write_all(&header.encode())?;
    let mut buf = Vec::with_capacity(header.record_size() * 4096);
    let mut n = 0u64;
    for ray in rays {
        encode_ray(header, &ray, &mut buf);
        n += 1;
        if buf.len() >= header.record_size() * 4096 {
            w.write_all(&buf)?;
            buf.clear();
        }
    }
    w.write_all(&buf)?;
    w.write_all(&LT_END)?;
    w.flush()?;
    Ok(n)
}

/// Convenience for the common case: a TM-25 header and its rays, in memory.
pub fn to_lighttools_bytes(h: &Header, rays: &[Ray]) -> Result<Vec<u8>> {
    let lt = LightToolsHeader::from_tm25(h)?;
    let mut out = Vec::with_capacity(LT_HEADER_SIZE + rays.len() * lt.record_size() + LT_END.len());
    write_lighttools(&mut out, &lt, rays.iter().copied())?;
    Ok(out)
}

/// A parsed LightTools file: header and decoded rays. Reading exists for
/// tests and round trips; the format carries far less than TM-25, so the
/// rays come back as `Ray` with only the exported columns set.
#[derive(Clone, Debug, PartialEq)]
pub struct LightToolsFile {
    pub header: LightToolsHeader,
    pub rays: Vec<Ray>,
}

impl LightToolsFile {
    /// The file as a TM-25 header and rays, ready for [`crate::write_tm25`]
    /// or any TM-25 consumer. See [`LightToolsHeader::to_tm25_header`].
    pub fn to_tm25(&self, name: &str) -> (Header, Vec<Ray>) {
        let size = self.header.file_size_for(self.rays.len() as u64);
        (
            self.header.to_tm25_header(name, Some(size)),
            self.rays.clone(),
        )
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = LightToolsHeader::parse(bytes)?;
        if header.far_field {
            return Err(Tm25Error::InvalidHeader(
                "LightTools far-field file without start points".into(),
            ));
        }
        let rec = header.record_size();
        let body = bytes.get(LT_HEADER_SIZE..).unwrap_or(&[]);
        let Some(end) = body.len().checked_sub(LT_END.len()) else {
            return Err(Tm25Error::Truncated {
                needed: LT_HEADER_SIZE + LT_END.len(),
                have: bytes.len(),
            });
        };
        if body[end..] != LT_END {
            return Err(Tm25Error::InvalidHeader("missing LTRFEND trailer".into()));
        }
        if end % rec != 0 {
            return Err(Tm25Error::SizeMismatch {
                ray_start: LT_HEADER_SIZE,
                expected: (end / rec * rec) as u64,
                actual: end as u64,
            });
        }
        let f = |r: &[u8], i: usize| {
            f32::from_le_bytes([r[4 * i], r[4 * i + 1], r[4 * i + 2], r[4 * i + 3]])
        };
        let rays = body[..end]
            .chunks_exact(rec)
            .map(|r| {
                let mut ray = Ray::new([f(r, 0), f(r, 1), f(r, 2)], [f(r, 3), f(r, 4), f(r, 5)]);
                match header.flux_kind {
                    FluxKind::Photometric => ray.luminous_flux_lm = Some(f(r, 6)),
                    _ => ray.radiant_flux_w = Some(f(r, 6)),
                }
                if header.per_ray_wavelength {
                    ray.wavelength_nm = Some(f(r, 7));
                }
                ray
            })
            .collect();
        Ok(Self { header, rays })
    }
}

// ---------------------------------------------------------------------------
// Spectrum: LightTools .sre, histogram from rays, sampled wavelengths
// ---------------------------------------------------------------------------

/// A LightTools `.sre` spectral distribution file for a spectral table: the
/// text format vendors ship next to their ray files (`dfat 1.0`, a data
/// name, `continuous`, `radiometric`, then tab-separated wavelength and
/// value pairs normalised to a peak of 1, CRLF line ends). This is the
/// companion to a `.ray` exported without colour information: LightTools
/// users assign the spectrum to the ray data source and can still change
/// it later, which per-ray wavelengths would not allow.
pub fn write_sre(t: &SpectralTable, name: &str) -> String {
    let peak = t.values.iter().cloned().fold(0.0f32, f32::max);
    let scale = if peak > 0.0 { 1.0 / peak } else { 1.0 };
    let name = name.replace(['\r', '\n', '\t'], " ");
    let mut s = String::with_capacity(64 + t.len() * 18);
    s.push_str(&format!(
        "#\t{name}\r\ndfat 1.0\t\r\ndataname:\t{name}\r\ncontinuous\t\r\nradiometric\t\r\n"
    ));
    for (w, v) in t.wavelengths_nm.iter().zip(&t.values) {
        s.push_str(&format!("{w:.4}\t{:.4}\r\n", v * scale));
    }
    s
}

/// Flux-weighted spectrum of rays that carry a wavelength each (spectral
/// identifier 2), binned to `bin_nm`: a [`SpectralTable`] with the bin
/// centres and the flux per bin, so such a file gets a spectrum to show and
/// to write as `.sre`. `None` when no ray has a wavelength.
pub fn spectrum_from_rays<'a, I: IntoIterator<Item = &'a Ray>>(
    rays: I,
    kind: FluxKind,
    bin_nm: f32,
) -> Option<SpectralTable> {
    let bin = if bin_nm > 0.0 { bin_nm } else { 5.0 };
    let mut sums: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
    for r in rays {
        let Some(w) = r.wavelength_nm else { continue };
        if !w.is_finite() {
            continue;
        }
        *sums.entry((w / bin).floor() as i64).or_default() += r.flux(kind) as f64;
    }
    let (&lo, _) = sums.first_key_value()?;
    let (&hi, _) = sums.last_key_value()?;
    let mut t = SpectralTable::default();
    for k in lo..=hi {
        t.wavelengths_nm.push((k as f32 + 0.5) * bin);
        t.values.push(sums.get(&k).copied().unwrap_or(0.0) as f32);
    }
    Some(t)
}

/// Draws wavelengths distributed like a spectral table, treating it as a
/// piecewise-linear density between its points.
#[derive(Clone, Debug)]
pub struct SpectrumSampler {
    w: Vec<f64>,
    v: Vec<f64>,
    /// Cumulative area up to each interval end, normalised to 1.
    cdf: Vec<f64>,
}

impl SpectrumSampler {
    /// `None` for a table with fewer than two points or no positive values.
    pub fn new(t: &SpectralTable) -> Option<Self> {
        if t.len() < 2 || t.wavelengths_nm.len() != t.values.len() {
            return None;
        }
        let w: Vec<f64> = t.wavelengths_nm.iter().map(|x| *x as f64).collect();
        let v: Vec<f64> = t.values.iter().map(|x| (*x as f64).max(0.0)).collect();
        let mut cdf = Vec::with_capacity(w.len() - 1);
        let mut acc = 0.0;
        for i in 0..w.len() - 1 {
            acc += (v[i] + v[i + 1]) * 0.5 * (w[i + 1] - w[i]).max(0.0);
            cdf.push(acc);
        }
        if acc <= 0.0 {
            return None;
        }
        for c in &mut cdf {
            *c /= acc;
        }
        Some(Self { w, v, cdf })
    }

    /// Wavelength in nm for a uniform `u` in `[0, 1)`.
    pub fn sample(&self, u: f64) -> f32 {
        let u = u.clamp(0.0, 1.0 - 1e-12);
        let i = self
            .cdf
            .partition_point(|c| *c <= u)
            .min(self.cdf.len() - 1);
        let c0 = if i == 0 { 0.0 } else { self.cdf[i - 1] };
        let span = (self.cdf[i] - c0).max(1e-300);
        // Position within the interval, inverting the linear density a→b.
        let t = ((u - c0) / span).clamp(0.0, 1.0);
        let (a, b) = (self.v[i], self.v[i + 1]);
        let x = if (a - b).abs() < 1e-12 * (a + b).max(1e-300) {
            t
        } else {
            // F(x) = (a x + (b - a) x²/2) / ((a + b)/2) = t  →  solve for x.
            let s = (a + b) * 0.5;
            let disc = (a * a + 2.0 * (b - a) * s * t).max(0.0);
            ((disc.sqrt() - a) / (b - a)).clamp(0.0, 1.0)
        };
        (self.w[i] + (self.w[i + 1] - self.w[i]) * x) as f32
    }
}

/// Write a LightTools file **with a wavelength per ray drawn from the
/// file's shared spectral table** (spectral identifier 3): colour info 2,
/// radiometric flux. The spectrum is then hard-wired into the rays in
/// LightTools and cannot be reassigned there, which is why the plain export
/// plus a `.sre` is the default and this is the option for sources whose
/// spectrum varies over position and angle. Deterministic for a given
/// `seed`. Errors when the file has no table or no radiant flux.
pub fn write_lighttools_sampled<W: Write, I: IntoIterator<Item = Ray>>(
    w: W,
    h: &Header,
    rays: I,
    seed: u64,
) -> Result<u64> {
    let table = h.shared_spectrum().ok_or_else(|| {
        Tm25Error::InvalidHeader("no shared spectral table to sample from".into())
    })?;
    let sampler = SpectrumSampler::new(table)
        .ok_or_else(|| Tm25Error::InvalidHeader("spectral table has no usable values".into()))?;
    if !h.flags.radiant_flux {
        return Err(Tm25Error::InvalidHeader(
            "LightTools takes a wavelength per ray only with radiant flux".into(),
        ));
    }
    let lt = LightToolsHeader {
        per_ray_wavelength: true,
        ..LightToolsHeader::from_tm25(h)?
    };
    let mut rng = crate::sample::Rng64::new(seed);
    write_lighttools(
        w,
        &lt,
        rays.into_iter()
            .map(move |r| r.with_wavelength(sampler.sample(rng.next_f64()))),
    )
}

/// In-memory form of [`write_lighttools_sampled`].
pub fn to_lighttools_bytes_sampled(h: &Header, rays: &[Ray], seed: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(LT_HEADER_SIZE + rays.len() * 32 + LT_END.len());
    write_lighttools_sampled(&mut out, h, rays.iter().copied(), seed)?;
    Ok(out)
}
