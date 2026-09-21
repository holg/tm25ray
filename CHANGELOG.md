# Changelog

## [0.1.0] - unreleased

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
