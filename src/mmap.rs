//! Memory-mapped native fast path (`mmap` feature, never on wasm32).

use std::path::Path;

use crate::error::Result;
use crate::header::ParseOptions;
use crate::reader::Tm25File;

/// A file mapped into memory; hand out zero-copy [`Tm25File`] views.
pub struct Tm25Mmap {
    map: memmap2::Mmap,
}

impl Tm25Mmap {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: the mapping is read-only and the file is not modified by
        // this crate while mapped; callers are expected not to truncate it.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Ok(Self { map })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    pub fn file(&self) -> Result<Tm25File<'_>> {
        Tm25File::parse(&self.map)
    }

    pub fn file_with(&self, opts: &ParseOptions) -> Result<Tm25File<'_>> {
        Tm25File::parse_with(&self.map, opts)
    }
}
