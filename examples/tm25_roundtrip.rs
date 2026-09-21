//! Verify that a file's header survives a parse/write round trip byte for byte.
//!
//!     cargo run --release --example tm25_roundtrip -- file.TM25RAY

use tm25ray::{write_tm25, Tm25File};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: tm25_roundtrip <file.TM25RAY>");
        std::process::exit(2);
    };
    let bytes = std::fs::read(&path).expect("read");
    let file = Tm25File::parse(&bytes).expect("parse");
    let h = &file.header;
    println!("{path}");
    println!(
        "  {} rays, record {} B, ray block at {}, name block {}",
        h.n_rays,
        h.record_size(),
        h.ray_start,
        h.has_name_block
    );

    // Header only: write with no rays (the writer fills in n_rays from the
    // slice, so restore the source count before comparing) and diff the prefix.
    let mut out = Vec::new();
    write_tm25(&mut out, h, &[]).expect("write");
    out[20..28].copy_from_slice(&h.n_rays.to_le_bytes());
    let n = h.ray_start;
    if out.len() < n {
        println!(
            "  MISMATCH: written header is {} B, source {} B",
            out.len(),
            n
        );
        std::process::exit(1);
    }
    let mut diffs = 0usize;
    let mut first = None;
    for i in 0..n {
        if out[i] != bytes[i] {
            diffs += 1;
            first.get_or_insert(i);
        }
    }
    if diffs == 0 {
        println!("  header round trip: {n} bytes identical");
    } else {
        println!(
            "  header round trip: {diffs} of {n} bytes differ, first at {}",
            first.unwrap()
        );
        let i = first.unwrap();
        let lo = i.saturating_sub(8);
        println!("    source {:02x?}", &bytes[lo..(i + 8).min(n)]);
        println!("    written {:02x?}", &out[lo..(i + 8).min(out.len())]);
        std::process::exit(1);
    }
}
