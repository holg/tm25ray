//! Time the reader on a real file: header, decode-only, far-field binning and
//! the reservoir subsample, for the streaming path and (with `--features mmap`)
//! the memory-mapped one.
//!
//!     cargo run --release --example tm25_bench -- file.TM25RAY [repeats]
//!
//! Throughput is rays per second over the whole file; the wall time includes
//! reading the bytes from the page cache, so run it twice and take the second.

use std::io::BufReader;
use std::time::Instant;

use tm25ray::{FarFieldBuilder, FluxKind, Reservoir, Tm25Reader};

const CHUNK: usize = 1 << 16;

fn report(label: &str, secs: f64, rays: u64, bytes: u64) {
    println!(
        "  {label:<28} {secs:>7.3} s   {:>8.1} M rays/s   {:>7.0} MB/s",
        rays as f64 / secs / 1e6,
        bytes as f64 / secs / 1e6
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: tm25_bench <file.TM25RAY> [repeats]");
        std::process::exit(2);
    };
    let repeats: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);

    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let open = || {
        let f = std::fs::File::open(&path).expect("open");
        Tm25Reader::new(BufReader::with_capacity(1 << 20, f)).expect("header")
    };

    let t0 = Instant::now();
    let h = open().header().clone();
    let header_ms = t0.elapsed().as_secs_f64() * 1e3;
    let n = h.n_rays;
    let kind = h.flux_kind();

    println!("{path}");
    println!(
        "  {n} rays, {} B/record, {:.1} MB, {:?}, header in {header_ms:.2} ms",
        h.record_size(),
        bytes as f64 / 1e6,
        kind
    );

    for pass in 1..=repeats {
        println!("pass {pass}:");

        // Decode only: every record turned into a Ray, nothing accumulated.
        let t = Instant::now();
        let mut r = open();
        let mut count = 0u64;
        let mut acc = 0f64;
        loop {
            let chunk = r.read_chunk(CHUNK).expect("chunk");
            if chunk.is_empty() {
                break;
            }
            count += chunk.len() as u64;
            acc += chunk[0].kz as f64; // keep the decode from being optimised out
        }
        assert_eq!(count, n);
        report("stream + decode", t.elapsed().as_secs_f64(), n, bytes);
        std::hint::black_box(acc);

        // Far field: solid-angle-normalised C/γ binning of every ray.
        let t = Instant::now();
        let mut r = open();
        let mut ff = FarFieldBuilder::new(10.0, 5.0, kind);
        loop {
            let chunk = r.read_chunk(CHUNK).expect("chunk");
            if chunk.is_empty() {
                break;
            }
            for ray in &chunk {
                ff.add(ray);
            }
        }
        let ff = ff.finish();
        report("+ far field (10°/5°)", t.elapsed().as_secs_f64(), n, bytes);

        // Reservoir: keep 200k flux-rescaled rays, what the viewer does.
        let t = Instant::now();
        let mut r = open();
        let mut keep = Reservoir::new(200_000, 1);
        loop {
            let chunk = r.read_chunk(CHUNK).expect("chunk");
            if chunk.is_empty() {
                break;
            }
            keep.extend(chunk);
        }
        let kept = keep.finish();
        report("+ reservoir 200k", t.elapsed().as_secs_f64(), n, bytes);

        #[cfg(feature = "mmap")]
        {
            let t = Instant::now();
            let map = tm25ray::Tm25Mmap::open(&path).expect("mmap");
            let file = map.file().expect("parse");
            let mut acc = 0f64;
            let mut count = 0u64;
            for ray in file.rays.iter() {
                acc += ray.kz as f64;
                count += 1;
            }
            assert_eq!(count, n);
            report("mmap + decode", t.elapsed().as_secs_f64(), n, bytes);
            std::hint::black_box(acc);
        }

        println!(
            "  peak {:.4} {:?}, half intensity {:?}°, kept {}",
            ff.azimuthal_average().iter().cloned().fold(0.0, f64::max),
            ff.unit,
            ff.half_intensity_gamma().map(|g| g.round()),
            kept.len()
        );
        viewer_pipeline(&path, kind, n, bytes);
    }
}

// The combination the Bevy viewer runs per file: far field over every ray plus
// a 200k reservoir, fed in 4 MB slices like the browser's Blob.slice loop.
#[allow(dead_code)]
fn viewer_pipeline(path: &str, kind: FluxKind, n: u64, bytes: u64) {
    let t = Instant::now();
    let f = std::fs::File::open(path).expect("open");
    let mut r = Tm25Reader::new(BufReader::with_capacity(4 << 20, f)).expect("header");
    let mut ff = FarFieldBuilder::new(10.0, 5.0, kind);
    let mut keep = Reservoir::new(200_000, 1);
    loop {
        let chunk = r.read_chunk(CHUNK).expect("chunk");
        if chunk.is_empty() {
            break;
        }
        for ray in &chunk {
            ff.add(ray);
        }
        keep.extend(chunk);
    }
    let ff = ff.finish().smoothed(1, 1, 12.0);
    let kept = keep.finish();
    std::hint::black_box((&ff, kept.len()));
    report(
        "viewer pipeline (ff+res)",
        t.elapsed().as_secs_f64(),
        n,
        bytes,
    );
}
