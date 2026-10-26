//! Convert between TM-25 and LightTools binary ray files (`.ray`), by magic,
//! and write the spectrum as a LightTools `.sre`.
//!
//!     cargo run --release --example tm25_to_lighttools -- in.TM25RAY out.ray [n_rays] [--sampled]
//!     cargo run --release --example tm25_to_lighttools -- in.TM25RAY out.sre
//!     cargo run --release --example tm25_to_lighttools -- in.ray out.TM25RAY [n_rays]
//!
//! With `n_rays` a flux-preserving subsample of that size is written
//! instead of every ray. TM-25 → LightTools exports radiant flux when the
//! file has it, luminous flux otherwise, and keeps a per-ray wavelength
//! (spectral identifier 2); a shared spectral table (identifier 3) goes
//! into the `.sre` instead, or, with `--sampled`, is drawn into the rays
//! as a wavelength each. LightTools → TM-25 is the reverse mapping; the
//! wavelength bounds are left unknown. See `src/lighttools.rs`.

use std::io::BufWriter;

use tm25ray::{
    is_lighttools, subsample, write_lighttools, write_lighttools_sampled, write_sre, write_tm25,
    LightToolsFile, LightToolsHeader, Tm25File,
};

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let sampled = std::env::args().any(|a| a == "--sampled");
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--sampled")
        .collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: tm25_to_lighttools <in.TM25RAY|in.ray> <out.ray|out.sre|out.TM25RAY> [n_rays] [--sampled]");
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
    if output.to_lowercase().ends_with(".sre") {
        let h = &file.header;
        let table = match h.shared_spectrum() {
            Some(t) => t.clone(),
            None => tm25ray::spectrum_from_rays(
                file.rays.iter().collect::<Vec<_>>().iter(),
                h.flux_kind(),
                5.0,
            )
            .unwrap_or_else(|| {
                die("the file has no spectral table and no wavelength per ray".into())
            }),
        };
        let name = h.text.name().trim();
        let name = if name.is_empty() {
            input.as_str()
        } else {
            name
        };
        std::io::Write::write_all(&mut { out }, write_sre(&table, name).as_bytes())
            .unwrap_or_else(|e| die(format!("write failed: {e}")));
        println!(
            "wrote {output}: {} points, {:.0} to {:.0} nm",
            table.len(),
            table.wavelengths_nm.first().unwrap_or(&0.0),
            table.wavelengths_nm.last().unwrap_or(&0.0)
        );
        return;
    }
    if sampled {
        let n = match keep.filter(|k| *k < file.rays.len()) {
            Some(k) => write_lighttools_sampled(out, &file.header, subsample(&file.rays, k, 1), 1),
            None => write_lighttools_sampled(out, &file.header, file.rays.iter(), 1),
        }
        .unwrap_or_else(|e| die(format!("sampled export failed: {e}")));
        println!("wrote {output}: {n} rays, radiometric, a wavelength per ray drawn from the shared spectral table");
        return;
    }
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
