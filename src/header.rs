//! TM-25-13 header: fixed 36 288-byte block followed by the variable
//! spectral / column-name / additional-text blocks.
//!
//! Field names carry the spec section numbers used by the reference
//! implementation (JuliusMuschaweck/TM25RaySetTools). Offsets, sizes and
//! the official field names were checked against Table 4-2 of the standard
//! in October 2026; what is still marked `VERIFY` was not settled by it.

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
/// Width of the creation date/time field (4.7.1.7) in bytes.
pub const DATE_TIME_LEN: usize = 28;
/// Each additional-column name (4.7.5) is a fixed 512-byte UTF-32LE string.
pub const COLUMN_NAME_BYTES: usize = 512;
/// Code points per column name.
pub const COLUMN_NAME_CHARS: usize = COLUMN_NAME_BYTES / 4;
/// The spectral-table block (4.7.4) and the additional text block (4.7.6)
/// are padded with zeros to a multiple of this many bytes.
pub const BLOCK_ALIGN: usize = 32;

/// Zero bytes that follow a block of `bytes` to reach the next multiple of
/// [`BLOCK_ALIGN`].
pub(crate) fn spectral_padding(bytes: usize) -> usize {
    (BLOCK_ALIGN - bytes % BLOCK_ALIGN) % BLOCK_ALIGN
}

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

/// The nine description strings (4.7.3.1 – 4.7.3.9), named as in Table 4-2
/// of the standard.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TextFields {
    pub raw: [String; TEXT_FIELDS],
}

impl TextFields {
    /// Name of the light source (4.7.3.1).
    pub fn name(&self) -> &str {
        &self.raw[0]
    }
    /// Manufacturer of the light source (4.7.3.2).
    pub fn manufacturer(&self) -> &str {
        &self.raw[1]
    }
    /// Creator of the optical light source model (4.7.3.3). A person or
    /// company, not a type code: ams OSRAM files carry initials here.
    pub fn model_creator(&self) -> &str {
        &self.raw[2]
    }
    /// Creator of the ray file (4.7.3.4): the measuring laboratory or the
    /// simulation author.
    pub fn rayfile_creator(&self) -> &str {
        &self.raw[3]
    }
    /// Measurement equipment / simulation software (4.7.3.5).
    pub fn equipment(&self) -> &str {
        &self.raw[4]
    }
    /// Camera information (4.7.3.6).
    pub fn camera(&self) -> &str {
        &self.raw[5]
    }
    /// Light source operation information, e.g. drive current and
    /// temperature (4.7.3.7).
    pub fn operating_condition(&self) -> &str {
        &self.raw[6]
    }
    /// Additional information (4.7.3.8).
    pub fn additional_info(&self) -> &str {
        &self.raw[7]
    }
    /// Data reference to the light source geometry (4.7.3.9), e.g. where the
    /// ray file origin sits relative to the package.
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

/// Column order implied by the known-data flags, as in Table 4-2 of the
/// standard (4.8.1): position, direction, radiant flux / S0, wavelength,
/// luminous flux / Y, S1 S2 S3, polarisation ellipse axis, X and Z
/// tristimulus, spectrum index, user-defined items.
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
///
/// Construct with `..ParseOptions::default()` rather than listing every field,
/// so that later additions stay source compatible.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    /// Reject flag combinations the reference implementation rejects
    /// (e.g. no flux column at all). Default `true`.
    pub strict_flags: bool,
    /// Total size of the file in bytes, when the caller knows it.
    ///
    /// Not needed to locate the ray block any more (the layout is fixed by
    /// the header), kept so `Tm25File::parse` and the memory-mapped reader
    /// can check `ray_start + n_rays × record_size` against it.
    pub total_size: Option<u64>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            strict_flags: true,
            total_size: None,
        }
    }
}

/// Parsed TM-25 header.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// 2013 for a TM-25 file; [`crate::lighttools::LIGHTTOOLS_VERSION_TAG`]
    /// for a header synthesised from a LightTools file.
    pub version: i32,
    /// 4.7.1.3. The reference code expects 0 or 1 (0 = simulation) and only
    /// warns otherwise; ams OSRAM files carry −1. Enumeration VERIFY.
    pub creation_method: i32,
    /// 4.7.1.4, lm. Zero when unknown (a NaN in the file is normalised to 0).
    pub luminous_flux_lm: f32,
    /// 4.7.1.5, W. Zero when unknown (a NaN in the file is normalised to 0).
    pub radiant_flux_w: f32,
    /// 4.7.1.6.
    pub n_rays: u64,
    /// 4.7.1.7, trimmed of NUL padding.
    pub date_time: String,
    /// 4.7.1.8, ray start position flag; observed 0 in all samples (VERIFY meaning).
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
    /// 4.7.1.15: size of the additional text block (4.7.6) in **bytes**, as
    /// stored at offset 84. The writer recomputes it from `additional_text`,
    /// padded to a multiple of 32.
    pub additional_text_size: i32,
    pub flags: KnownDataFlags,
    pub text: TextFields,
    /// 4.7.4, `n_spectra` tables (4.7.1.13).
    pub spectra: Vec<SpectralTable>,
    /// 4.7.5, names of the additional columns, one per `n_additional_items`.
    pub column_names: Vec<String>,
    /// 4.7.6.
    pub additional_text: String,
    /// Raw bits of the two flux totals as they appeared in the file, so an
    /// unknown-value sentinel is reproduced byte for byte on write even though
    /// the public fields read 0.0. `None` for headers built in code.
    pub(crate) raw_flux_bits: Option<(u32, u32)>,
    /// The exact NaN bit pattern this file uses for "unknown" f32 fields
    /// (ams OSRAM writes `0x7FC00001`, Lumileds `0x7F800001`, the Rust
    /// default is `0x7FC00000`). Only needed so a round trip is byte exact.
    pub nan_pattern: u32,
    /// Column layout of one ray record, derived from the flags.
    pub columns: Vec<Column>,
    /// Byte offset of the first ray record.
    pub ray_start: usize,
}

impl Header {
    /// True for a header synthesised from a LightTools ray file
    /// ([`crate::LightToolsHeader::to_tm25_header`]): its ray block is
    /// followed by the 7-byte `LTRFEND` trailer, so the file is that much
    /// longer than `ray_start + ray_block_size()`.
    pub fn is_lighttools(&self) -> bool {
        self.version == crate::lighttools::LIGHTTOOLS_VERSION_TAG
    }

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
            raw_flux_bits: None,
            nan_pattern: f32::NAN.to_bits(),
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
        // Unknown totals are written as NaN (Lumileds uses the signalling
        // 0x7F800001, ams OSRAM writes 0.0). Normalise to 0.0 so callers can
        // do arithmetic without every product turning into NaN; `has_*_flux`
        // tells the two apart.
        let raw_luminous = r.f32()?;
        let raw_radiant = r.f32()?;
        let raw_flux_bits = Some((raw_luminous.to_bits(), raw_radiant.to_bits()));
        let luminous_flux_lm = zero_if_unknown(raw_luminous);
        let radiant_flux_w = zero_if_unknown(raw_radiant);
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
        let raw_single = r.f32()?;
        let raw_min = r.f32()?;
        let raw_max = r.f32()?;
        // Remember the vendor's "unknown" sentinel so the writer reproduces it.
        let nan_pattern = [raw_single, raw_min, raw_max]
            .iter()
            .find(|v| is_unknown(**v))
            .map(|v| v.to_bits())
            .unwrap_or_else(|| f32::NAN.to_bits());
        let single_wavelength_nm = opt_f32(raw_single);
        let min_wavelength_nm = opt_f32(raw_min);
        let max_wavelength_nm = opt_f32(raw_max);
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
            // Each table is its point count followed by the pairs; no
            // explicit table index is stored (Table 4-2).
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
        // The spectral-table block is padded with zeros to a multiple of
        // 32 bytes. Table 4-2 of the standard only says "variable"; the
        // reference implementation pads on read and write, and the vendor
        // files agree: the ams OSRAM 51-point table (412 bytes) is followed
        // by 4 zero bytes, a Lumileds file with no table by none. Versions
        // up to 0.1.1 of this crate read those 4 bytes as an optional
        // "column-name count", which happened to fit both vendors.
        let spectral_bytes = r.pos - FIXED_HEADER_SIZE;
        r.take(spectral_padding(spectral_bytes))?;
        // Column names (4.7.5): one fixed 512-byte UTF-32LE string per
        // additional item, NUL padded. No count and no per-name length.
        let mut column_names = Vec::with_capacity(n_additional_items as usize);
        for _ in 0..n_additional_items {
            column_names.push(r.utf32_fixed(COLUMN_NAME_CHARS, "column name")?);
        }
        // Additional text (4.7.6): `additional_text_size` bytes of UTF-32LE.
        // The reference requires a multiple of 32; any multiple of 4 is read.
        if additional_text_size % 4 != 0 {
            return Err(Tm25Error::InvalidHeader(format!(
                "additional text size {additional_text_size} is not a multiple of 4"
            )));
        }
        let additional_text =
            r.utf32_exact(additional_text_size as usize / 4, "additional text")?;

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
            raw_flux_bits,
            nan_pattern,
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
    "model creator",
    "ray file creator",
    "equipment",
    "camera",
    "operating condition",
    "additional info",
    "data reference",
];

/// True for the bit patterns vendors use to mean "not known".
///
/// Normally that is a NaN written little-endian like every other field
/// (ams OSRAM: `01 00 C0 7F`). Lumileds LUXEON files instead write the
/// sentinel **byte-swapped** (`7F 80 00 01`), which read little-endian is not
/// a NaN at all but the denormal 2.36e-38 — small enough to look like a real
/// measurement and poison anything derived from it. The ray data in those same
/// files is ordinary little-endian, so this is a quirk of the sentinel only.
///
/// Matched exactly, never by bit class: a value is "unknown" only if it is a
/// real NaN or precisely that one swapped pattern.
fn is_unknown(v: f32) -> bool {
    // Exact patterns only. An earlier version asked "would the swapped bytes
    // be a NaN?", which also matched ~0.4 % of ordinary positive values and
    // would have silently zeroed real measurements.
    const BYTE_SWAPPED_SENTINEL: u32 = 0x0100_807F;
    v.is_nan() || v.to_bits() == BYTE_SWAPPED_SENTINEL
}

fn zero_if_unknown(v: f32) -> f32 {
    if is_unknown(v) {
        0.0
    } else {
        v
    }
}

fn opt_f32(v: f32) -> Option<f32> {
    if is_unknown(v) {
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
