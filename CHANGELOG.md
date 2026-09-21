# Changelog

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
