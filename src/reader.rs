//! Zero-copy and streaming readers.

use std::io::Read;

use crate::error::{Result, Tm25Error};
use crate::header::{Header, ParseOptions, FIXED_HEADER_SIZE};
use crate::ray::{Ray, RayLayout, RayView};

/// A whole file in memory: header plus a zero-copy ray view.
#[derive(Clone, Debug)]
pub struct Tm25File<'a> {
    pub header: Header,
    pub rays: RayView<'a>,
}

impl<'a> Tm25File<'a> {
    /// Parse with default (strict) options. The slice must hold the whole
    /// file: the ray block size is checked against the header.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        Self::parse_with(bytes, &ParseOptions::default())
    }

    pub fn parse_with(bytes: &'a [u8], opts: &ParseOptions) -> Result<Self> {
        // The slice is the whole file, so the trailer question can be settled
        // by size rather than guessed (see `ParseOptions::total_size`).
        let mut opts = *opts;
        opts.total_size.get_or_insert(bytes.len() as u64);
        let header = Header::parse(bytes, &opts)?;
        let expected = header.ray_block_size();
        let actual = (bytes.len() - header.ray_start) as u64;
        if expected != actual {
            return Err(Tm25Error::SizeMismatch {
                ray_start: header.ray_start,
                expected,
                actual,
            });
        }
        let n = usize::try_from(header.n_rays)
            .map_err(|_| Tm25Error::InvalidHeader("ray count exceeds address space".into()))?;
        let layout = RayLayout::from_header(&header);
        let rays = RayView::new(layout, &bytes[header.ray_start..], n);
        Ok(Self { header, rays })
    }
}

/// Streaming reader over any `Read`: parses the header, then yields rays in
/// chunks without holding the file in memory. Use it for multi-GB files and
/// for browser `Blob.slice` feeds.
pub struct Tm25Reader<R: Read> {
    header: Header,
    layout: RayLayout,
    inner: R,
    remaining: u64,
    delivered: u64,
    buf: Vec<u8>,
}

impl<R: Read> Tm25Reader<R> {
    pub fn new(inner: R) -> Result<Self> {
        Self::with_options(inner, &ParseOptions::default())
    }

    /// Stream from any reader. The header is parsed from a growing prefix, so
    /// the total size is unknown unless `opts.total_size` says otherwise; set
    /// it (from `File::metadata` or a `Content-Length`) when a producer might
    /// omit the optional 4.7.5/4.7.6 trailer *and* write an empty one is
    /// indistinguishable — see [`ParseOptions::total_size`].
    pub fn with_options(mut inner: R, opts: &ParseOptions) -> Result<Self> {
        // Grow the buffer until the header parses; `Truncated` tells us how much.
        let mut buf = Vec::with_capacity(FIXED_HEADER_SIZE + 4096);
        let mut needed = FIXED_HEADER_SIZE + 4;
        let header = loop {
            if buf.len() < needed {
                let old = buf.len();
                buf.resize(needed, 0);
                let mut filled = old;
                while filled < needed {
                    let n = inner.read(&mut buf[filled..needed])?;
                    if n == 0 {
                        return Err(Tm25Error::Truncated {
                            needed,
                            have: filled,
                        });
                    }
                    filled += n;
                }
            }
            match Header::parse(&buf, opts) {
                Ok(h) => break h,
                Err(Tm25Error::Truncated { needed: more, .. }) if more > buf.len() => needed = more,
                Err(e) => return Err(e),
            }
        };
        // Anything read past the header already belongs to the ray block.
        let leftover = buf.split_off(header.ray_start);
        let layout = RayLayout::from_header(&header);
        let remaining = header.n_rays;
        Ok(Self {
            header,
            layout,
            inner,
            remaining,
            delivered: 0,
            buf: leftover,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn layout(&self) -> &RayLayout {
        &self.layout
    }

    /// Rays not yet delivered.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Read up to `max` rays. Returns an empty vector at the end of the ray
    /// block, and an error if the stream ends early.
    pub fn read_chunk(&mut self, max: usize) -> Result<Vec<Ray>> {
        if self.remaining == 0 || max == 0 {
            return Ok(Vec::new());
        }
        let n = (self.remaining.min(max as u64)) as usize;
        let rs = self.layout.record_size;
        let want = n * rs;
        if self.buf.len() < want {
            let old = self.buf.len();
            self.buf.resize(want, 0);
            let mut filled = old;
            while filled < want {
                let got = self.inner.read(&mut self.buf[filled..want])?;
                if got == 0 {
                    let complete = filled / rs;
                    return Err(Tm25Error::RayCountMismatch {
                        expected: self.header.n_rays,
                        actual: self.delivered + complete as u64,
                    });
                }
                filled += got;
            }
        }
        let out: Vec<Ray> = self.buf[..want]
            .chunks_exact(rs)
            .map(|r| self.layout.decode(r))
            .collect();
        self.buf.drain(..want);
        self.remaining -= n as u64;
        self.delivered += n as u64;
        Ok(out)
    }

    /// Consume the reader as an iterator of rays (chunked internally).
    pub fn rays(self) -> RayStream<R> {
        RayStream {
            reader: self,
            chunk: Vec::new().into_iter(),
            chunk_size: 65_536,
        }
    }
}

/// Iterator adapter over [`Tm25Reader::read_chunk`].
pub struct RayStream<R: Read> {
    reader: Tm25Reader<R>,
    chunk: std::vec::IntoIter<Ray>,
    chunk_size: usize,
}

impl<R: Read> RayStream<R> {
    pub fn header(&self) -> &Header {
        self.reader.header()
    }
}

impl<R: Read> Iterator for RayStream<R> {
    type Item = Result<Ray>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(r) = self.chunk.next() {
            return Some(Ok(r));
        }
        match self.reader.read_chunk(self.chunk_size) {
            Ok(v) if v.is_empty() => None,
            Ok(v) => {
                self.chunk = v.into_iter();
                self.chunk.next().map(Ok)
            }
            Err(e) => Some(Err(e)),
        }
    }
}
