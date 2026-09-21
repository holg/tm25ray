//! Print the header, spectrum and a 10° far-field profile of a TM-25 file,
//! streaming it so multi-GB files work.
//!
//!     cargo run --release -p tm25ray --example tm25_info -- file.TM25RAY [reservoir_n]

use std::io::BufReader;

use tm25ray::{FarFieldBuilder, Reservoir, Tm25Reader};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: tm25_info <file.TM25RAY> [reservoir_n]");
        std::process::exit(2);
    };
    let keep: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);

    let t0 = std::time::Instant::now();
    let file = std::fs::File::open(&path).expect("open");
    let mut reader = Tm25Reader::new(BufReader::with_capacity(1 << 20, file)).expect("header");
    let h = reader.header().clone();

    println!("{}", path);
    println!(
        "  {} rays, record {} B, ray block at {}, {:?}",
        h.n_rays,
        h.record_size(),
        h.ray_start,
        h.flux_kind()
    );
    println!(
        "  radiant {} W, luminous {} lm, created {} (method {})",
        h.radiant_flux_w, h.luminous_flux_lm, h.date_time, h.creation_method
    );
    println!(
        "  {} / {} / {} / {}",
        h.text.name(),
        h.text.manufacturer(),
        h.text.operating_condition(),
        h.text.data_reference()
    );
    println!(
        "  spectral id {:?}, {}..{} nm, {} table(s), columns {:?}",
        h.spectral_id,
        h.min_wavelength_nm.unwrap_or(f32::NAN),
        h.max_wavelength_nm.unwrap_or(f32::NAN),
        h.spectra.len(),
        h.columns
    );
    if let Some(t) = h.shared_spectrum() {
        println!(
            "  spectrum: {} points, peak {:?}, centroid {:.1} nm, UV {}",
            t.len(),
            t.peak(),
            t.centroid_nm().unwrap_or(f32::NAN),
            t.is_ultraviolet()
        );
    }

    let kind = h.flux_kind();
    let mut ff = FarFieldBuilder::new(10.0, 10.0, kind);
    let mut res = Reservoir::new(keep, 1);
    let mut n = 0u64;
    loop {
        let chunk = reader.read_chunk(1 << 16).expect("read");
        if chunk.is_empty() {
            break;
        }
        for r in &chunk {
            ff.add(r);
        }
        n += chunk.len() as u64;
        res.extend(chunk);
    }
    let ff = ff.finish();
    let kept = res.finish();
    let dt = t0.elapsed();
    println!(
        "  streamed {} rays in {:.2} s ({:.1} M rays/s), kept {} (flux {:.4})",
        n,
        dt.as_secs_f64(),
        n as f64 / dt.as_secs_f64() / 1e6,
        kept.len(),
        kept.iter().map(|r| r.flux(kind) as f64).sum::<f64>()
    );
    let prof = ff.azimuthal_average();
    let max = prof.iter().cloned().fold(0.0, f64::max);
    println!(
        "  far field ({:?}): peak {:.4}, half intensity at {:?}°",
        ff.unit,
        max,
        ff.half_intensity_gamma().map(|g| g.round())
    );
    print!("  γ:   ");
    for g in ff.g_angles().iter().take(10) {
        print!("{:>6.0}", g);
    }
    println!();
    print!("  I/I0:");
    for v in prof.iter().take(10) {
        print!("{:>6.2}", v / max);
    }
    println!();
}
