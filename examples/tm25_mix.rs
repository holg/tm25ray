//! Mix channel ray files (e.g. ams OSRAM blue + yellow of one white LED)
//! into one TM-25 file.
//!
//!     cargo run --release --example tm25_mix -- out.TM25RAY blue.TM25RAY[=w] yellow.TM25RAY[=w] [--indexed] [--keep N] [--fractions 0.035,0.965 [--radiometric]] [--spectra blue.txt,yellow.txt] [--fit spectrum.txt]
//!
//! Weights multiply each channel's flux (default 1). `--fractions` sets the
//! channels' shares of the summed flux, photometric unless `--radiometric`,
//! which is how vendors tabulate the split per colour bin. `--spectra`
//! replaces the files' spectral tables by the given ones (`nm value`
//! text files, one per channel): a bin's own spectra go with the bin's
//! split, since the phosphor spectrum changes from bin to bin. `--fit` finds the
//! weights that reproduce a target spectrum (tab- or space-separated
//! `nm value` lines, as the vendors' spectrum .txt files) from the channel
//! spectra by least squares, scaled so the mix keeps the summed radiant
//! power of the inputs. `--indexed` writes identifier 4 (tables kept, index
//! per ray, radiometric); the default is one combined table (identifier 3).
//! `--keep N` subsamples the result to N rays, flux preserved.

use std::io::BufWriter;

use tm25ray::{
    fit_weights, mix, subsample, write_tm25, Channel, MixMode, RayLayout, RayView, SpectralTable,
    Tm25File,
};

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn read_spectrum_txt(path: &str) -> SpectralTable {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("cannot read {path}: {e}")));
    let mut t = SpectralTable::default();
    for line in text.lines() {
        let mut it = line.split(['\t', ' ', ';', ',']).filter(|s| !s.is_empty());
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            if let (Ok(nm), Ok(v)) = (a.parse::<f32>(), b.parse::<f32>()) {
                t.wavelengths_nm.push(nm);
                t.values.push(v);
            }
        }
    }
    if t.len() < 2 {
        die(format!("{path}: no 'nm value' pairs found"));
    }
    t
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut indexed = false;
    let mut keep: Option<usize> = None;
    let mut fit: Option<String> = None;
    let mut fractions: Option<Vec<f64>> = None;
    let mut radiometric = false;
    let mut spectra_files: Option<Vec<String>> = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--indexed" => indexed = true,
            "--keep" => {
                i += 1;
                keep = raw.get(i).and_then(|s| s.parse().ok());
            }
            "--fit" => {
                i += 1;
                fit = raw.get(i).cloned();
            }
            "--fractions" => {
                i += 1;
                fractions = raw
                    .get(i)
                    .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect());
            }
            "--radiometric" => radiometric = true,
            "--spectra" => {
                i += 1;
                spectra_files = raw
                    .get(i)
                    .map(|s| s.split(',').map(|v| v.trim().to_string()).collect());
            }
            a => positional.push(a.to_string()),
        }
        i += 1;
    }
    if positional.len() < 3 {
        eprintln!("usage: tm25_mix <out.TM25RAY> <a.TM25RAY[=w]> <b.TM25RAY[=w]> ... [--indexed] [--keep N] [--fit spectrum.txt]");
        std::process::exit(2);
    }
    let output = &positional[0];
    let mut files = Vec::new();
    let mut weights = Vec::new();
    for spec in &positional[1..] {
        let (path, w) = match spec.rsplit_once('=') {
            Some((p, w)) if w.parse::<f64>().is_ok() => (p.to_string(), w.parse::<f64>().unwrap()),
            _ => (spec.clone(), 1.0),
        };
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| die(format!("cannot read {path}: {e}")));
        let f = Tm25File::parse(&bytes).unwrap_or_else(|e| die(format!("{path}: {e}")));
        let rays: Vec<_> = f.rays.iter().collect();
        let stem = std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        const COLOURS: [&str; 10] = [
            "blue", "yellow", "red", "green", "amber", "white", "cyan", "lime", "phosphor", "uv",
        ];
        let label = stem
            .split(['_', '-', ' '])
            .find(|t| COLOURS.contains(&t.to_lowercase().as_str()))
            .map(|t| t.to_string())
            .unwrap_or(stem);
        files.push((f.header, rays, label));
        weights.push(w);
    }
    let overrides: Vec<Option<SpectralTable>> = match spectra_files {
        Some(list) => {
            if list.len() != files.len() {
                die(format!(
                    "--spectra needs {} files, got {}",
                    files.len(),
                    list.len()
                ));
            }
            list.iter().map(|p| Some(read_spectrum_txt(p))).collect()
        }
        None => vec![None; files.len()],
    };
    if let Some(fr) = fractions {
        if fr.len() != files.len() {
            die(format!(
                "--fractions needs {} values, got {}",
                files.len(),
                fr.len()
            ));
        }
        let fluxes: Vec<f64> = files
            .iter()
            .map(|(h, _, _)| {
                if radiometric {
                    h.radiant_flux_w as f64
                } else {
                    h.luminous_flux_lm as f64
                }
            })
            .collect();
        weights = tm25ray::weights_for_flux_fractions(&fluxes, &fr);
        for ((_, _, l), (f, w)) in files.iter().zip(fr.iter().zip(&weights)) {
            println!(
                "{l}: {:.1} % of the {} flux → weight {w:.4}",
                f * 100.0,
                if radiometric { "radiant" } else { "luminous" }
            );
        }
    }
    if let Some(target) = fit {
        let target = read_spectrum_txt(&target);
        let spectra: Vec<&SpectralTable> = files
            .iter()
            .zip(&overrides)
            .map(|((h, _, l), o)| {
                o.as_ref()
                    .or_else(|| h.shared_spectrum())
                    .unwrap_or_else(|| die(format!("channel {l} has no spectral table to fit")))
            })
            .collect();
        let w = fit_weights(&target, &spectra);
        // Radiant power of each channel as in its file, to turn power
        // fractions into flux multipliers that keep the total power.
        let powers: Vec<f64> = files
            .iter()
            .map(|(h, _, _)| {
                if h.flags.radiant_flux || h.radiant_flux_w > 0.0 {
                    h.radiant_flux_w as f64
                } else {
                    0.0
                }
            })
            .collect();
        let total: f64 = powers.iter().sum();
        let wsum: f64 = w.iter().sum::<f64>().max(1e-30);
        for (i, (p, wi)) in powers.iter().zip(&w).enumerate() {
            weights[i] = if *p > 0.0 { wi / wsum * total / p } else { 0.0 };
            println!(
                "{}: {:.1} % of the radiant power → weight {:.4} on the file's {:.4} W",
                files[i].2,
                wi / wsum * 100.0,
                weights[i],
                p
            );
        }
    }
    let channels: Vec<Channel> = files
        .iter()
        .zip(&weights)
        .zip(&overrides)
        .map(|(((h, rays, label), w), o)| Channel {
            header: h,
            rays,
            weight: *w,
            label: label.clone(),
            spectrum: o.clone(),
        })
        .collect();
    let mode = if indexed {
        MixMode::Indexed
    } else {
        MixMode::Shared
    };
    let (h, rays) = mix(&channels, mode).unwrap_or_else(|e| die(format!("mix failed: {e}")));
    let rays = match keep.filter(|k| *k < rays.len()) {
        Some(k) => {
            let layout = RayLayout::from_header(&h);
            let mut buf = Vec::new();
            for r in &rays {
                layout.encode(r, &mut buf);
            }
            subsample(&RayView::new(layout, &buf, rays.len()), k, 1)
        }
        None => rays,
    };
    let out = BufWriter::new(
        std::fs::File::create(output)
            .unwrap_or_else(|e| die(format!("cannot write {output}: {e}"))),
    );
    write_tm25(out, &h, &rays).unwrap_or_else(|e| die(format!("write failed: {e}")));
    println!(
        "wrote {output}: {} rays, {:?}, {:.3} lm / {:.4} W, spectral id {:?}, {} table(s){}",
        rays.len(),
        h.flux_kind(),
        h.luminous_flux_lm,
        h.radiant_flux_w,
        h.spectral_id,
        h.spectra.len(),
        h.shared_spectrum()
            .map(|t| format!(
                ", combined spectrum {:.0}–{:.0} nm peak at {:.0} nm",
                t.wavelengths_nm[0],
                t.wavelengths_nm.last().unwrap(),
                t.peak().map(|p| p.0).unwrap_or(0.0)
            ))
            .unwrap_or_default()
    );
}
