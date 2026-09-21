//! TM-25-13 header: fixed 36 288-byte block followed by the variable
//! spectral / column-name / additional-text blocks.
//!
//! Field names carry the spec section numbers used by the reference
//! implementation (JuliusMuschaweck/TM25RaySetTools). Everything marked
//! `VERIFY` was inferred from real files and the reference code, not from
//! the paywalled standard text.

use crate::error::{Result, Tm25Error};

/// Magic bytes at offset 0.
pub const MAGIC: [u8; 4] = *b"TM25";
/// The only format version this crate understands.
pub const VERSION_2013: i32 = 2013;
/// Size of the fixed header block (4.7.1) including reserved bytes.
pub const HEADER_BLOCK_SIZE: usize = 256;
/// Offset of the known-data-flags block (4.7.2): eight little-endian i32.
pub const FLAGS_OFFSET: usize = 256;
/// Offset of the nine description strings (4.7.3).
pub const TEXT_OFFSET: usize = 288;
/// Code points per description string, UTF-32LE, NUL padded.
pub const TEXT_FIELD_CHARS: usize = 1000;
/// Number of description strings.
pub const TEXT_FIELDS: usize = 9;
/// Offset of the first variable block (spectral tables, 4.7.4).
pub const FIXED_HEADER_SIZE: usize = TEXT_OFFSET + TEXT_FIELDS * TEXT_FIELD_CHARS * 4; // 36 288
/// Width of the creation date/time field in bytes. VERIFY: 28 is inferred
/// from the ray-start-position field following at 56; the vendor writes 19
/// ASCII characters and zero padding, so 28 vs 32 is invisible in samples.
pub const DATE_TIME_LEN: usize = 28;

/// Spectral data identifier (4.7.1.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectralId {
    /// 0 — no spectral information.
    None,
    /// 1 — one wavelength for the whole file (`single_wavelength_nm`).
    SingleWavelength,
    /// 2 — every ray carries its own wavelength column.
    PerRayWavelength,
    /// 3 — one spectral table in the header applies to every ray.
    SharedTable,
    /// 4 — several tables; every ray carries a table index column.
    PerRayTableIndex,
    /// Anything else (kept so unknown files still parse leniently).
    Unknown(i32),
}

impl SpectralId {
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => SpectralId::None,
            1 => SpectralId::SingleWavelength,
            2 => SpectralId::PerRayWavelength,
            3 => SpectralId::SharedTable,
            4 => SpectralId::PerRayTableIndex,
            other => SpectralId::Unknown(other),
        }
    }

    pub fn to_i32(self) -> i32 {
        match self {
            SpectralId::None => 0,
            SpectralId::SingleWavelength => 1,
            SpectralId::PerRayWavelength => 2,
            SpectralId::SharedTable => 3,
            SpectralId::PerRayTableIndex => 4,
            SpectralId::Unknown(v) => v,
        }
    }
}

/// Known data flags block (4.7.2): which per-ray columns exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KnownDataFlags {
    pub position: bool,
    pub direction: bool,
    pub radiant_flux: bool,
    pub wavelength: bool,
    pub luminous_flux: bool,
    pub stokes: bool,
    pub tristimulus: bool,
    pub spectrum_index: bool,
}

impl KnownDataFlags {
    /// The minimal, most common layout: position, direction, radiant flux.
    pub fn radiometric() -> Self {
        Self {
            position: true,
            direction: true,
            radiant_flux: true,
            ..Default::default()
        }
    }

    /// Position, direction, luminous flux.
    pub fn photometric() -> Self {
        Self {
            position: true,
            direction: true,
            luminous_flux: true,
            ..Default::default()
        }
    }

    fn from_words(w: &[i32; 8]) -> Self {
        Self {
            position: w[0] != 0,
            direction: w[1] != 0,
            radiant_flux: w[2] != 0,
            wavelength: w[3] != 0,
            luminous_flux: w[4] != 0,
            stokes: w[5] != 0,
            tristimulus: w[6] != 0,
            spectrum_index: w[7] != 0,
        }
    }

    pub(crate) fn to_words(self) -> [i32; 8] {
        [
            self.position as i32,
            self.direction as i32,
            self.radiant_flux as i32,
            self.wavelength as i32,
            self.luminous_flux as i32,
            self.stokes as i32,
            self.tristimulus as i32,
            self.spectrum_index as i32,
        ]
    }
}

/// Which flux a file carries, derived from the flags and the header totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FluxKind {
    /// Per-ray radiant flux in W; header radiant total.
    Radiometric,
    /// Per-ray luminous flux in lm; header luminous total.
    Photometric,
    /// Both columns present; `Ray::flux` prefers radiant.
    Both,
}

/// The nine description strings (4.7.3.1 – 4.7.3.9). Order is confirmed
/// from real files; the official wording of each name is VERIFY.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TextFields {
    pub raw: [String; TEXT_FIELDS],
}

impl TextFields {
    /// Name of the light source (4.7.3.1).
    pub fn name(&self) -> &str {
        &self.raw[0]
    }
    /// Manufacturer (4.7.3.2).
    pub fn manufacturer(&self) -> &str {
        &self.raw[1]
    }
    /// Model / type code (4.7.3.3, VERIFY).
    pub fn model(&self) -> &str {
        &self.raw[2]
    }
    /// Measurement laboratory or simulation author (4.7.3.4).
    pub fn laboratory(&self) -> &str {
        &self.raw[3]
    }
    /// Measurement equipment / simulation software (4.7.3.5).
    pub fn equipment(&self) -> &str {
        &self.raw[4]
    }
    /// Camera / detector information (4.7.3.6).
    pub fn camera(&self) -> &str {
        &self.raw[5]
    }
    /// Operating condition, e.g. drive current (4.7.3.7).
    pub fn operating_condition(&self) -> &str {
        &self.raw[6]
    }
    /// Additional information (4.7.3.8).
    pub fn additional_info(&self) -> &str {
        &self.raw[7]
    }
    /// Data reference (4.7.3.9).
    pub fn data_reference(&self) -> &str {
        &self.raw[8]
    }
}

/// One spectral table (4.7.4): interleaved `(wavelength nm, value)` pairs on disk.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SpectralTable {
    pub wavelengths_nm: Vec<f32>,
    pub values: Vec<f32>,
}

impl SpectralTable {
    pub fn len(&self) -> usize {
        self.wavelengths_nm.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wavelengths_nm.is_empty()
    }

    /// `(wavelength, value)` of the largest value.
    pub fn peak(&self) -> Option<(f32, f32)> {
        self.wavelengths_nm
            .iter()
            .zip(&self.values)
            .map(|(&w, &v)| (w, v))
            .fold(None, |best, cur| match best {
                Some((_, bv)) if bv >= cur.1 => best,
                _ => Some(cur),
            })
    }

    /// Value-weighted mean wavelength in nm.
    pub fn centroid_nm(&self) -> Option<f32> {
        let sum: f32 = self.values.iter().sum();
        if sum <= 0.0 {
            return None;
        }
        let m: f32 = self
            .wavelengths_nm
            .iter()
            .zip(&self.values)
            .map(|(w, v)| w * v)
            .sum();
        Some(m / sum)
    }

    /// True when the table lies (by weight) below 400 nm.
    pub fn is_ultraviolet(&self) -> bool {
        let total: f32 = self.values.iter().sum();
        if total <= 0.0 {
            return false;
        }
        let uv: f32 = self
            .wavelengths_nm
            .iter()
            .zip(&self.values)
            .filter(|(w, _)| **w < 400.0)
            .map(|(_, v)| v)
            .sum();
        uv / total > 0.9
    }
}

/// One `f32` column of a ray record, in file order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    X,
    Y,
    Z,
    Kx,
    Ky,
    Kz,
    RadiantFlux,
    Wavelength,
    LuminousFlux,
    S1,
    S2,
    S3,
    PolX,
    PolY,
    PolZ,
    TriX,
    TriZ,
    SpectrumIndex,
    /// User-defined item `i`, named by `Header::column_names[i]` when present.
    Additional(usize),
}

/// Column order implied by the known-data flags. VERIFY against the
/// standard: this follows the item order of the reference implementation.
pub fn columns_from_flags(flags: &KnownDataFlags, n_additional: usize) -> Vec<Column> {
    let mut c = vec![
        Column::X,
        Column::Y,
        Column::Z,
        Column::Kx,
        Column::Ky,
        Column::Kz,
    ];
    if flags.radiant_flux {
        c.push(Column::RadiantFlux);
    }
    if flags.wavelength {
        c.push(Column::Wavelength);
    }
    if flags.luminous_flux {
        c.push(Column::LuminousFlux);
    }
    if flags.stokes {
        c.extend([
            Column::S1,
            Column::S2,
            Column::S3,
            Column::PolX,
            Column::PolY,
            Column::PolZ,
        ]);
    }
    if flags.tristimulus {
        c.extend([Column::TriX, Column::TriZ]);
    }
    if flags.spectrum_index {
        c.push(Column::SpectrumIndex);
    }
    for i in 0..n_additional {
        c.push(Column::Additional(i));
    }
    c
}

/// Parsing knobs.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    /// Reject flag combinations the reference implementation rejects
    /// (e.g. no flux column at all). Default `true`.
    pub strict_flags: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self { strict_flags: true }
    }
}

/// Parsed TM-25 header.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub version: i32,
    /// 4.7.1.3. Reference code: 0 = simulation. Vendor files show −1
    /// (presumably "not specified"). Enumeration VERIFY.
    pub creation_method: i32,
    /// 4.7.1.4, lm. Zero or NaN when unknown.
    pub luminous_flux_lm: f32,
    /// 4.7.1.5, W. Zero or NaN when unknown.
    pub radiant_flux_w: f32,
    /// 4.7.1.6.
    pub n_rays: u64,
    /// 4.7.1.7, trimmed of NUL padding.
    pub date_time: String,
    /// 4.7.1.8, observed 0 in all samples (VERIFY meaning).
    pub start_position: i32,
    /// 4.7.1.9.
    pub spectral_id: SpectralId,
    /// 4.7.1.10, `None` when the file stores NaN.
    pub single_wavelength_nm: Option<f32>,
    /// 4.7.1.11.
    pub min_wavelength_nm: Option<f32>,
    /// 4.7.1.12.
    pub max_wavelength_nm: Option<f32>,
    /// 4.7.1.14: user-defined per-ray columns.
    pub n_additional_items: usize,
    /// Size of the additional text block. VERIFY: assumed at offset 84 and
    /// counted in UTF-32 code points.
    pub additional_text_size: i32,
    pub flags: KnownDataFlags,
    pub text: TextFields,
    /// 4.7.4, `n_spectra` tables (4.7.1.13).
    pub spectra: Vec<SpectralTable>,
    /// 4.7.5, names of the additional columns.
    pub column_names: Vec<String>,
    /// 4.7.6.
    pub additional_text: String,
    /// Column layout of one ray record, derived from the flags.
    pub columns: Vec<Column>,
    /// Byte offset of the first ray record.
    pub ray_start: usize,
}

impl Header {
    /// Bytes per ray record.
    pub fn record_size(&self) -> usize {
        self.columns.len() * 4
    }

    /// Total size of the ray block promised by the header.
    pub fn ray_block_size(&self) -> u64 {
        self.n_rays * self.record_size() as u64
    }

    /// Which flux the rays carry.
    pub fn flux_kind(&self) -> FluxKind {
        match (self.flags.radiant_flux, self.flags.luminous_flux) {
            (true, true) => FluxKind::Both,
            (false, true) => FluxKind::Photometric,
            _ => FluxKind::Radiometric,
        }
    }

    /// Header total for the given kind.
    pub fn total_flux(&self, kind: FluxKind) -> f32 {
        match kind {
            FluxKind::Radiometric | FluxKind::Both => self.radiant_flux_w,
            FluxKind::Photometric => self.luminous_flux_lm,
        }
    }

    /// Index of a column in the record, if present.
    pub fn column_index(&self, col: Column) -> Option<usize> {
        self.columns.iter().position(|c| *c == col)
    }

    /// The table that applies to every ray (`SpectralId::SharedTable`).
    pub fn shared_spectrum(&self) -> Option<&SpectralTable> {
        match self.spectral_id {
            SpectralId::SharedTable => self.spectra.first(),
            _ => None,
        }
    }

    /// Build a header for writing. `n_rays`, `columns` and `ray_start` are
    /// filled in by the writer.
    pub fn new(flags: KnownDataFlags) -> Self {
        Self {
            version: VERSION_2013,
            creation_method: 0,
            luminous_flux_lm: 0.0,
            radiant_flux_w: 0.0,
            n_rays: 0,
            date_time: String::new(),
            start_position: 0,
            spectral_id: SpectralId::None,
            single_wavelength_nm: None,
            min_wavelength_nm: None,
            max_wavelength_nm: None,
            n_additional_items: 0,
            additional_text_size: 0,
            flags,
            text: TextFields::default(),
            spectra: Vec::new(),
            column_names: Vec::new(),
            additional_text: String::new(),
            columns: columns_from_flags(&flags, 0),
            ray_start: 0,
        }
    }

    /// Parse the header from the start of a file. Returns
    /// [`Tm25Error::Truncated`] with the number of bytes needed when the
    /// slice ends inside the header, so a streaming reader can grow its
    /// buffer and retry.
    pub fn parse(bytes: &[u8], opts: &ParseOptions) -> Result<Header> {
        let mut r = Cursor::new(bytes);
        let magic = r.bytes4()?;
        if magic != MAGIC {
            return Err(Tm25Error::BadMagic(magic));
        }
        let version = r.i32()?;
        if version != VERSION_2013 {
            return Err(Tm25Error::UnsupportedVersion(version));
        }
        let creation_method = r.i32()?;
        let luminous_flux_lm = r.f32()?;
        let radiant_flux_w = r.f32()?;
        let n_rays = r.u64()?;
        let date_raw = r.take(DATE_TIME_LEN)?;
        let date_time = date_raw
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as char)
            .collect::<String>()
            .trim()
            .to_string();
        let start_position = r.i32()?;
        let spectral_id = SpectralId::from_i32(r.i32()?);
        let single_wavelength_nm = opt_f32(r.f32()?);
        let min_wavelength_nm = opt_f32(r.f32()?);
        let max_wavelength_nm = opt_f32(r.f32()?);
        let n_spectra = r.i32()?;
        let n_additional_items = r.i32()?;
        let additional_text_size = r.i32()?;
        if n_spectra < 0 || n_additional_items < 0 || additional_text_size < 0 {
            return Err(Tm25Error::InvalidHeader(format!(
                "negative counts: spectra {n_spectra}, additional items {n_additional_items}, text {additional_text_size}"
            )));
        }
        if n_spectra > 10_000 || n_additional_items > 1_000 || additional_text_size > 10_000_000 {
            return Err(Tm25Error::InvalidHeader(format!(
                "implausible counts: spectra {n_spectra}, additional items {n_additional_items}, text {additional_text_size}"
            )));
        }

        r.seek(FLAGS_OFFSET)?;
        let mut words = [0i32; 8];
        for w in words.iter_mut() {
            *w = r.i32()?;
        }
        let flags = KnownDataFlags::from_words(&words);

        r.seek(TEXT_OFFSET)?;
        let mut text = TextFields::default();
        for (i, slot) in text.raw.iter_mut().enumerate() {
            *slot = r.utf32_fixed(TEXT_FIELD_CHARS, TEXT_FIELD_NAMES[i])?;
        }

        // Variable blocks ---------------------------------------------------
        r.seek(FIXED_HEADER_SIZE)?;
        let mut spectra = Vec::with_capacity(n_spectra as usize);
        for _ in 0..n_spectra {
            // VERIFY: no explicit table index is stored before the point
            // count in single-table files; multi-table files may differ.
            let n = r.i32()?;
            if !(0..=1_000_000).contains(&n) {
                return Err(Tm25Error::InvalidHeader(format!(
                    "implausible spectral table size {n}"
                )));
            }
            let mut t = SpectralTable {
                wavelengths_nm: Vec::with_capacity(n as usize),
                values: Vec::with_capacity(n as usize),
            };
            for _ in 0..n {
                t.wavelengths_nm.push(r.f32()?);
                t.values.push(r.f32()?);
            }
            spectra.push(t);
        }
        // Column names (4.7.5): count, then length-prefixed UTF-32 strings.
        let n_names = r.i32()?;
        if !(0..=1_000).contains(&n_names) {
            return Err(Tm25Error::InvalidHeader(format!(
                "implausible column name count {n_names}"
            )));
        }
        let mut column_names = Vec::with_capacity(n_names as usize);
        for _ in 0..n_names {
            let len = r.i32()?;
            if !(0..=10_000).contains(&len) {
                return Err(Tm25Error::InvalidHeader(format!(
                    "implausible column name length {len}"
                )));
            }
            column_names.push(r.utf32_exact(len as usize, "column name")?);
        }
        // Additional text (4.7.6). VERIFY: size assumed in code points.
        let additional_text = r.utf32_exact(additional_text_size as usize, "additional text")?;

        let ray_start = r.pos;
        let columns = columns_from_flags(&flags, n_additional_items as usize);

        let header = Header {
            version,
            creation_method,
            luminous_flux_lm,
            radiant_flux_w,
            n_rays,
            date_time,
            start_position,
            spectral_id,
            single_wavelength_nm,
            min_wavelength_nm,
            max_wavelength_nm,
            n_additional_items: n_additional_items as usize,
            additional_text_size,
            flags,
            text,
            spectra,
            column_names,
            additional_text,
            columns,
            ray_start,
        };
        if opts.strict_flags {
            header.validate()?;
        }
        Ok(header)
    }

    /// Consistency rules mirrored from the reference implementation.
    pub fn validate(&self) -> Result<()> {
        let f = &self.flags;
        let bad = |msg: &str| Err(Tm25Error::InvalidHeader(msg.to_string()));
        if !f.position || !f.direction {
            return bad("position and direction flags must be set");
        }
        if !f.radiant_flux && !f.luminous_flux {
            return bad("both radiant and luminous flux flags are missing");
        }
        match self.spectral_id {
            SpectralId::PerRayWavelength => {
                if !f.wavelength {
                    return bad("spectral id 2 (per-ray wavelength) requires the wavelength flag");
                }
                if !f.radiant_flux {
                    return bad("spectral id 2 requires the radiant flux flag");
                }
            }
            SpectralId::SharedTable => {
                if self.spectra.is_empty() {
                    return bad("spectral id 3 (shared table) but no spectral table");
                }
            }
            SpectralId::PerRayTableIndex => {
                if !f.spectrum_index {
                    return bad("spectral id 4 requires the spectrum index flag");
                }
                if !f.radiant_flux {
                    return bad("spectral id 4 requires the radiant flux flag");
                }
                if self.spectra.is_empty() {
                    return bad("spectral id 4 but no spectral tables");
                }
            }
            _ => {}
        }
        if f.spectrum_index && self.spectral_id != SpectralId::PerRayTableIndex {
            return bad("spectrum index flag requires spectral id 4");
        }
        if f.stokes && !f.radiant_flux {
            return bad("Stokes flag requires the radiant flux flag");
        }
        if f.tristimulus && (!f.luminous_flux || self.spectral_id != SpectralId::None) {
            return bad("tristimulus flag requires the luminous flux flag and spectral id 0");
        }
        for (i, t) in self.spectra.iter().enumerate() {
            if t.wavelengths_nm.iter().any(|w| w.is_nan() || *w <= 0.0) {
                return Err(Tm25Error::InvalidHeader(format!(
                    "spectral table {i} has a non-positive wavelength"
                )));
            }
            if t.values.iter().any(|v| v.is_nan() || *v < 0.0) {
                return Err(Tm25Error::InvalidHeader(format!(
                    "spectral table {i} has a negative or NaN value"
                )));
            }
        }
        Ok(())
    }
}

pub(crate) const TEXT_FIELD_NAMES: [&str; TEXT_FIELDS] = [
    "name",
    "manufacturer",
    "model",
    "laboratory",
    "equipment",
    "camera",
    "operating condition",
    "additional info",
    "data reference",
];

fn opt_f32(v: f32) -> Option<f32> {
    if v.is_nan() {
        None
    } else {
        Some(v)
    }
}

/// Bounds-checked little-endian cursor.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<()> {
        let needed = self.pos.saturating_add(n);
        if needed > self.bytes.len() {
            Err(Tm25Error::Truncated {
                needed,
                have: self.bytes.len(),
            })
        } else {
            Ok(())
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.bytes.len() {
            return Err(Tm25Error::Truncated {
                needed: pos,
                have: self.bytes.len(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn bytes4(&mut self) -> Result<[u8; 4]> {
        let s = self.take(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes4()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes4()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes4()?))
    }

    fn u64(&mut self) -> Result<u64> {
        let s = self.take(8)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(s);
        Ok(u64::from_le_bytes(b))
    }

    /// Fixed-width UTF-32LE field, NUL terminated inside `chars` code points.
    fn utf32_fixed(&mut self, chars: usize, field: &'static str) -> Result<String> {
        let start = self.pos;
        self.need(chars * 4)?;
        let mut out = String::new();
        for i in 0..chars {
            let cp = self.u32()?;
            if cp == 0 {
                break;
            }
            match char::from_u32(cp) {
                Some(c) => out.push(c),
                None => {
                    return Err(Tm25Error::InvalidText {
                        field,
                        code_point: cp,
                    });
                }
            }
            let _ = i;
        }
        self.pos = start + chars * 4;
        Ok(out)
    }

    /// Exactly `chars` UTF-32LE code points, NULs kept out of the result.
    fn utf32_exact(&mut self, chars: usize, field: &'static str) -> Result<String> {
        self.need(chars.saturating_mul(4))?;
        let mut out = String::with_capacity(chars);
        for _ in 0..chars {
            let cp = self.u32()?;
            if cp == 0 {
                continue;
            }
            match char::from_u32(cp) {
                Some(c) => out.push(c),
                None => {
                    return Err(Tm25Error::InvalidText {
                        field,
                        code_point: cp,
                    })
                }
            }
        }
        Ok(out)
    }
}
