//! Luminance map of a ray file from a view direction.
//!
//!     cargo run --release --example tm25_luminance -- file.TM25RAY [C_deg] [gamma_deg] [cone_deg] [pixel_mm]
//!
//! Prints the map's statistics and a coarse ASCII picture. For the
//! generated Lambertian ground truth (1 × 1 mm, 1 W) every lit interior
//! pixel should read 318 310 W/(sr·m²) within the Monte-Carlo noise the
//! map reports, from any direction. See `docs/luminance.md`.

use tm25ray::{luminance_map, Tm25File};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: tm25_luminance <file.TM25RAY> [C_deg] [gamma_deg] [cone_deg] [pixel_mm]");
        std::process::exit(2);
    };
    let num = |i: usize, d: f32| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (c, g, cone, pixel) = (num(1, 0.0), num(2, 0.0), num(3, 5.0), num(4, 0.05));
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1)
    });
    let file = Tm25File::parse(&bytes).unwrap_or_else(|e| {
        eprintln!("not a TM-25 file: {e}");
        std::process::exit(1)
    });
    let rays: Vec<_> = file.rays.iter().collect();
    let (cr, gr) = (c.to_radians(), g.to_radians());
    let dir = [gr.sin() * cr.cos(), gr.sin() * cr.sin(), gr.cos()];
    let Some(m) = luminance_map(&rays, file.header.flux_kind(), dir, cone, pixel, 96) else {
        println!("no ray within {cone}° of C = {c}°, γ = {g}°");
        std::process::exit(1);
    };
    println!(
        "{path}\nview C = {c}°, γ = {g}°, cone ±{cone}°, pixel {:.3} mm → {} × {} pixels, {} rays in the cone",
        m.pixel_mm, m.width, m.height, m.rays
    );
    println!(
        "peak {:.0} {u}, mean over {} lit pixels {:.0} {u}, {:.1} rays per lit pixel (≈ {:.0} % noise per pixel)",
        m.peak,
        m.lit_pixels,
        m.mean_lit,
        m.rays_per_lit_pixel,
        100.0 / m.rays_per_lit_pixel.max(1e-9).sqrt(),
        u = m.unit()
    );
    // Interior mean: pixels whose 8 neighbours are all lit.
    let lit = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < m.width
            && (y as usize) < m.height
            && m.get(x as usize, y as usize) > 0.0
    };
    let (mut sum, mut n) = (0.0, 0usize);
    for y in 0..m.height as isize {
        for x in 0..m.width as isize {
            if (-1..=1).all(|dy| (-1..=1).all(|dx| lit(x + dx, y + dy))) {
                sum += m.get(x as usize, y as usize);
                n += 1;
            }
        }
    }
    if n > 0 {
        println!(
            "interior mean over {n} pixels: {:.0} {}",
            sum / n as f64,
            m.unit()
        );
    }
    let shades = b" .:-=+*#%@";
    let step = (m.width / 48).max(1);
    for y in (0..m.height).rev().step_by(step * 2) {
        let line: String = (0..m.width)
            .step_by(step)
            .map(|x| shades[((m.get(x, y) / m.peak.max(1e-300)) * 9.0).round() as usize] as char)
            .collect();
        println!("  {line}");
    }
}
