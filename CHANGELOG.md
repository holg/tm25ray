# Changelog

## [0.2.0] - 2026-10-24

The block after the spectral tables was laid out wrong in 0.1.x, so files
this crate wrote were not readable by a standard-conforming reader and files
with additional columns could not be read. Fixed after checking the layout
against the standard's own table. Two public API changes, hence 0.2.

- **Breaking: the bytes after the spectral tables follow the standard.** The
  spectral-table block is zero padded to a multiple of 32 bytes, each
  additional-column name is a fixed 512-byte UTF-32 string, and the text-block
  size at offset 84 counts bytes. 0.1.x modelled the same bytes as an optional
  "name count + length-prefixed names" trailer. That fitted both vendors by
  coincidence (the ams OSRAM 51-point table needs exactly 4 bytes of padding,
  which reads as a count of zero; Lumileds has no table and no padding) but
  wrote files a standard reader finds 4 to 20 bytes off, and could not read
  any file with additional columns. `Header::has_name_block` is gone;
  `ParseOptions::total_size` only feeds the size check now. Checked against
  Table 4-2 of the standard and the C++ reference; three ams OSRAM packages
  (UV-C, GW CSSRMU blue and yellow, 100k to 5M rays, ray blocks at 36 704,
  37 184 and 37 664) and two Lumileds files round-trip byte for byte. The
  three files in `docs/sample/` are regenerated. Files written by 0.1.x no
  longer parse (size mismatch of 4 bytes); regenerate them.
- **Breaking: `TextFields::model` is now `model_creator`, `laboratory` is
  `rayfile_creator`**, after the official field names "Creator of Optical
  Light Source Model" (4.7.3.3) and "Creator of the Ray File" (4.7.3.4). The
  ams OSRAM value "BZ" in field 3 is a person's initials, not a type code.
- The spectrum index (identifier 4) is written as an i32, as the standard's
  table says. The f32 bit pattern the C++ reference writes is still read.
- The Lambertian ground-truth file carries a per-ray wavelength, uniform over
  360–830 nm, so luminance code can be checked with V(λ) as well as radiance
  code without it. Suggested by the TM-25 initiator, who also confirmed the
  equal-flux, cosine-weighted sampling as the right shape for a ground truth.
  32 MB instead of 28.
- The date field width (28 bytes), the text-size position (84), the absence
  of a per-table index in the spectral block and the nine text-field names
  are confirmed. The `VERIFY` list in `docs/format.md` is down to the
  creation-method enumeration and the start-position flag.

- `examples/tm25_sample.rs` writes a synthetic but physically modelled file
  (Lambertian chip, Snell refraction at a silicone dome, rim leakage,
  phosphor-white spectrum with per-ray wavelengths), so the format and the
  viewer can be tried without a vendor download. Two generated files are in
  `docs/sample/`; neither ships in the crate.
- The sample rays start where they leave the package (dome surface or rim
  gap) instead of on the chip, so start point and direction lie on one line
  in air. The chip-origin version was unusable for luminance. Both sample
  files are regenerated.
- `examples/tm25_lambertian.rs` and `docs/sample/lambertian_1mm_1M.TM25RAY`:
  a flat Lambertian square with exactly known radiance, as a ground truth for
  luminance code.
- `docs/luminance.md`: how to compute luminance from a ray file, with noise
  estimates and a numerical check.
- `Ray::with_wavelength` and `Ray::with_spectrum_index`, to match the existing
  flux builders.
- A landing page for GitHub Pages at holg.github.io/tm25ray.

## [0.1.1] - 2026-09-21

Vendor-compatibility fixes found on Lumileds LUXEON files, which 0.1.0 could
not open at all (`invalid header: implausible column name count -1089800634`).

- **Optional 4.7.5/4.7.6 trailer.** Column names and additional text are not
  written by every producer. Lumileds starts the ray block right after the
  fixed 36 288-byte header; ams OSRAM writes the trailer. The parser now
  detects which, and `Header::has_name_block` preserves it so a round trip
  stays byte exact. `ParseOptions::total_size` (set automatically by
  `Tm25File::parse` and the memory-mapped reader) resolves the one ambiguous
  case, an empty trailer versus a first ray at the origin.
- **Byte-swapped "unknown" sentinel.** Lumileds writes the unknown-value NaN
  with swapped bytes (`7F 80 00 01`), which little-endian decodes to the
  denormal 2.36e-38 rather than a NaN — a value small enough to pass for a
  real measurement. Flux totals and wavelength bounds now report 0.0 / `None`
  for it, matched on the exact bit pattern; the original bits are written back
  unchanged unless the caller edits the field.
- `ParseOptions` gained a field; construct it with `..Default::default()`.
- New `examples/tm25_roundtrip.rs` verifies a file's header reproduces byte for
  byte. All four ams OSRAM sizes and the Lumileds 5M file pass.


## [0.1.0] - 2026-09-21

First release, extracted from the `eulumdat-tm25` crate of eulumdat-rs.

- TM-25-13 header parser (fixed block, known-data flags, UTF-32 description
  strings, spectral tables, column names, additional text) with the
  consistency rules of the reference implementation.
- Zero-copy `Tm25File`, streaming `Tm25Reader`, optional `mmap` feature.
- All flag-driven record layouts; `RayLayout` decodes any column.
- Writer with byte-exact header round trip.
- Flux-preserving `subsample` and streaming `Reservoir`.
- `FarField` C/γ binning, bilinear sampling, smoothing, profiles.
- Coordinate helpers for EULUMDAT angles and y-up engines.
- `examples/tm25_bench.rs`: per-stage timings on a real file; measured figures
  are in the README (20M rays / 560 MB decode at ~91 M rays/s on an M2 Max).
