//! Convert between TM-25 and LightTools binary ray files (`.ray`), by magic.
//!
//!     cargo run --release --example tm25_to_lighttools -- in.TM25RAY out.ray [n_rays]
//!     cargo run --release --example tm25_to_lighttools -- in.ray out.TM25RAY [n_rays]
//!
//! With `n_rays` a flux-preserving subsample of that size is written
//! instead of every ray. TM-25 → LightTools exports radiant flux when the
//! file has it, luminous flux otherwise, and keeps a per-ray wavelength
//! (spectral identifier 2). LightTools → TM-25 is the reverse mapping; the
//! wavelength bounds are left unknown. See `src/lighttools.rs`.

use std::io::BufWriter;

use tm25ray::{
    is_lighttools, subsample, write_lighttools, write_tm25, LightToolsFile, LightToolsHeader,
    Tm25File,
};

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: tm25_to_lighttools <in.TM25RAY|in.ray> <out.ray|out.TM25RAY> [n_rays]");
        std::process::exit(2);
    };
    let keep: Option<usize> = args.get(2).and_then(|s| s.parse().ok());
    let bytes = std::fs::read(input).unwrap_or_else(|e| die(format!("cannot read {input}: {e}")));
    let out = BufWriter::new(
        std::fs::File::create(output)
            .unwrap_or_else(|e| die(format!("cannot write {output}: {e}"))),
    );
    if is_lighttools(&bytes) {
        let lt = LightToolsFile::parse(&bytes)
            .unwrap_or_else(|e| die(format!("not a LightTools file: {e}")));
        let name = std::path::Path::new(input)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (h, rays) = lt.to_tm25(&name);
        let rays = match keep.filter(|k| *k < rays.len()) {
            Some(k) => {
                let view = tm25ray::RayView::new(
                    tm25ray::RayLayout::from_header(&h),
                    &bytes[h.ray_start..],
                    rays.len(),
                );
                subsample(&view, k, 1)
            }
            None => rays,
        };
        write_tm25(out, &h, &rays).unwrap_or_else(|e| die(format!("write failed: {e}")));
        println!(
            "wrote {output}: {} rays as TM-25, {:?}, {} {}",
            rays.len(),
            h.flux_kind(),
            h.total_flux(h.flux_kind()),
            if h.flags.wavelength {
                "with a wavelength per ray"
            } else {
                "no wavelength"
            }
        );
        return;
    }
    let file = Tm25File::parse(&bytes).unwrap_or_else(|e| die(format!("not a TM-25 file: {e}")));
    let lt = LightToolsHeader::from_tm25(&file.header)
        .unwrap_or_else(|e| die(format!("cannot map to LightTools: {e}")));
    let n = match keep.filter(|k| *k < file.rays.len()) {
        Some(k) => write_lighttools(out, &lt, subsample(&file.rays, k, 1)),
        None => write_lighttools(out, &lt, file.rays.iter()),
    }
    .unwrap_or_else(|e| die(format!("write failed: {e}")));
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
