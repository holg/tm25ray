//! Quality report for a ray file: is it usable, and what is wrong with it?
//!
//!     cargo run --release --example tm25_quality -- file.TM25RAY [file.ray ...]
//!
//! Streams each file once (TM-25, or a LightTools `.ray` by magic) and
//! prints the report from `tm25ray::quality`. Exit status 1 if any file has
//! an error-level finding, so it can gate a build or an upload.

use std::io::BufReader;

use tm25ray::{
    is_lighttools, LightToolsFile, QualityAccumulator, QualityReport, Severity, Tm25Reader,
};

fn report(path: &str) -> Result<QualityReport, String> {
    let mut head = [0u8; 4];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let _ = f.read(&mut head);
    }
    if is_lighttools(&head) {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let lt = LightToolsFile::parse(&bytes).map_err(|e| e.to_string())?;
        let (h, rays) = lt.to_tm25(path);
        return Ok(tm25ray::quality_report(&h, &rays));
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader =
        Tm25Reader::new(BufReader::with_capacity(1 << 20, file)).map_err(|e| e.to_string())?;
    let mut acc = QualityAccumulator::new(reader.header());
    loop {
        let chunk = reader.read_chunk(65_536).map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            break;
        }
        for r in &chunk {
            acc.add(r);
        }
    }
    Ok(acc.finish())
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: tm25_quality <file.TM25RAY|file.ray> ...");
        std::process::exit(2);
    }
    let mut failed = false;
    for p in &paths {
        match report(p) {
            Ok(r) => {
                print!("{}", r.to_text(p));
                println!();
                if r.worst() == Some(Severity::Error) {
                    failed = true;
                }
            }
            Err(e) => {
                println!("{p}: cannot read: {e}\n");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
