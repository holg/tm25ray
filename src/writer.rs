//! TM-25-13 writer: the inverse of [`crate::Header::parse`], byte-for-byte
//! compatible with the vendor files this crate was verified against.

use std::io::Write;

use crate::error::{Result, Tm25Error};
use crate::header::{
    columns_from_flags, Header, DATE_TIME_LEN, FIXED_HEADER_SIZE, FLAGS_OFFSET, MAGIC,
    TEXT_FIELD_CHARS, TEXT_OFFSET, VERSION_2013,
};
use crate::ray::{Ray, RayLayout};

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_utf32(out: &mut Vec<u8>, s: &str) {
    for c in s.chars() {
        out.extend_from_slice(&(c as u32).to_le_bytes());
    }
}

/// Serialise the header (everything before the ray block). `n_rays` is
/// taken from the argument, not from `header.n_rays`.
pub fn encode_header(header: &Header, n_rays: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(FIXED_HEADER_SIZE + 4096);
    out.extend_from_slice(&MAGIC);
    put_i32(&mut out, VERSION_2013);
    put_i32(&mut out, header.creation_method);
    put_f32(&mut out, header.luminous_flux_lm);
    put_f32(&mut out, header.radiant_flux_w);
    out.extend_from_slice(&n_rays.to_le_bytes());
    let mut date = header.date_time.as_bytes().to_vec();
    if !date.is_ascii() {
        return Err(Tm25Error::InvalidHeader("date/time must be ASCII".into()));
    }
    date.resize(DATE_TIME_LEN, 0);
    if header.date_time.len() > DATE_TIME_LEN {
        return Err(Tm25Error::InvalidHeader(format!(
            "date/time longer than {DATE_TIME_LEN} bytes"
        )));
    }
    out.extend_from_slice(&date);
    put_i32(&mut out, header.start_position);
    put_i32(&mut out, header.spectral_id.to_i32());
    put_f32(&mut out, header.single_wavelength_nm.unwrap_or(f32::NAN));
    put_f32(&mut out, header.min_wavelength_nm.unwrap_or(f32::NAN));
    put_f32(&mut out, header.max_wavelength_nm.unwrap_or(f32::NAN));
    put_i32(&mut out, header.spectra.len() as i32);
    put_i32(&mut out, header.n_additional_items as i32);
    let text_chars = header.additional_text.chars().count() as i32;
    put_i32(&mut out, text_chars);
    out.resize(FLAGS_OFFSET, 0);
    for w in header.flags.to_words() {
        put_i32(&mut out, w);
    }
    debug_assert_eq!(out.len(), TEXT_OFFSET);
    for (i, s) in header.text.raw.iter().enumerate() {
        let start = out.len();
        let n = s.chars().count();
        if n > TEXT_FIELD_CHARS - 1 {
            return Err(Tm25Error::InvalidHeader(format!(
                "text field {i} has {n} characters, limit is {}",
                TEXT_FIELD_CHARS - 1
            )));
        }
        put_utf32(&mut out, s);
        out.resize(start + TEXT_FIELD_CHARS * 4, 0);
    }
    debug_assert_eq!(out.len(), FIXED_HEADER_SIZE);
    for t in &header.spectra {
        if t.wavelengths_nm.len() != t.values.len() {
            return Err(Tm25Error::InvalidHeader(
                "spectral table wavelengths/values length mismatch".into(),
            ));
        }
        put_i32(&mut out, t.wavelengths_nm.len() as i32);
        for (w, v) in t.wavelengths_nm.iter().zip(&t.values) {
            put_f32(&mut out, *w);
            put_f32(&mut out, *v);
        }
    }
    put_i32(&mut out, header.column_names.len() as i32);
    for name in &header.column_names {
        put_i32(&mut out, name.chars().count() as i32);
        put_utf32(&mut out, name);
    }
    put_utf32(&mut out, &header.additional_text);
    Ok(out)
}

/// Write a complete file: header, then one record per ray in the column
/// order implied by the header flags and `n_additional_items`.
pub fn write_tm25<W: Write>(mut w: W, header: &Header, rays: &[Ray]) -> Result<()> {
    let bytes = encode_header(header, rays.len() as u64)?;
    w.write_all(&bytes)?;
    let layout =
        RayLayout::from_columns(columns_from_flags(&header.flags, header.n_additional_items));
    let mut buf = Vec::with_capacity(layout.record_size * 4096);
    for chunk in rays.chunks(4096) {
        buf.clear();
        for r in chunk {
            layout.encode(r, &mut buf);
        }
        w.write_all(&buf)?;
    }
    w.flush()?;
    Ok(())
}

/// Convenience: the whole file as bytes.
pub fn to_bytes(header: &Header, rays: &[Ray]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_tm25(&mut out, header, rays)?;
    Ok(out)
}
