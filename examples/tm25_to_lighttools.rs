//! Convert a TM-25 ray file to a LightTools binary ray file (`.ray`).
//!
//!     cargo run --release --example tm25_to_lighttools -- in.TM25RAY out.ray [n_rays]
//!
//! With `n_rays` a flux-preserving subsample of that size is written
//! instead of every ray. Radiant flux is exported when the file has it,
//! luminous flux otherwise; a per-ray wavelength (spectral identifier 2) is
//! kept. See `src/lighttools.rs` for the layout.

use std::io::BufWriter;

use tm25ray::{subsample, write_lighttools, LightToolsHeader, Tm25File};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: tm25_to_lighttools <in.TM25RAY> <out.ray> [n_rays]");
        std::process::exit(2);
    };
    let keep: Option<usize> = args.get(2).and_then(|s| s.parse().ok());
    let bytes = std::fs::read(input).unwrap_or_else(|e| {
        eprintln!("cannot read {input}: {e}");
        std::process::exit(1)
    });
    let file = Tm25File::parse(&bytes).unwrap_or_else(|e| {
        eprintln!("not a TM-25 file: {e}");
        std::process::exit(1)
    });
    let lt = LightToolsHeader::from_tm25(&file.header).unwrap_or_else(|e| {
        eprintln!("cannot map to LightTools: {e}");
        std::process::exit(1)
    });
    let out = BufWriter::new(std::fs::File::create(output).unwrap_or_else(|e| {
        eprintln!("cannot write {output}: {e}");
        std::process::exit(1)
    }));
    let n = match keep.filter(|k| *k < file.rays.len()) {
        Some(k) => write_lighttools(out, &lt, subsample(&file.rays, k, 1)),
        None => write_lighttools(out, &lt, file.rays.iter()),
    }
    .unwrap_or_else(|e| {
        eprintln!("write failed: {e}");
        std::process::exit(1)
    });
    println!(
        "wrote {output}: {n} rays, {:?}, {} {}, {}",
        lt.flux_kind,
        lt.total_flux,
        if lt.flux_kind == tm25ray::FluxKind::Photometric {
            "lm"
        } else {
            "W"
        },
        if lt.per_ray_wavelength {
            "wavelength per ray"
        } else {
            "no colour info"
        }
    );
}
