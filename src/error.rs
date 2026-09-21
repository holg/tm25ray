use thiserror::Error;

/// Everything that can go wrong while reading or writing a TM-25 file.
#[derive(Debug, Error)]
pub enum Tm25Error {
    #[error("file truncated: need {needed} bytes, have {have}")]
    Truncated { needed: usize, have: usize },

    #[error("not a TM-25 file: magic {0:?}")]
    BadMagic([u8; 4]),

    #[error("unsupported TM-25 version {0} (only 2013 is supported)")]
    UnsupportedVersion(i32),

    #[error(
        "ray block size mismatch: header promises {expected} bytes of rays from offset {ray_start}, file has {actual}"
    )]
    SizeMismatch {
        ray_start: usize,
        expected: u64,
        actual: u64,
    },

    #[error("ray count mismatch: header promises {expected} rays, stream delivered {actual}")]
    RayCountMismatch { expected: u64, actual: u64 },

    #[error("invalid header: {0}")]
    InvalidHeader(String),

    #[error("invalid UTF-32 text in {field}: code point {code_point:#x}")]
    InvalidText {
        field: &'static str,
        code_point: u32,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Tm25Error>;
