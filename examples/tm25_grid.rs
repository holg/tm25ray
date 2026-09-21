//! Dump the binned C/γ far field of a file as a table (rows C, columns γ).
use tm25ray::{FarField, FluxKind, Tm25File};
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).unwrap();
    let f = Tm25File::parse(&bytes).unwrap();
    let rays: Vec<_> = f.rays.iter().collect();
    let ff = FarField::from_rays(&rays, 10.0, 5.0, FluxKind::Radiometric);
    let max = ff.max_intensity();
    print!("C\\γ  ");
    for g in ff.g_angles().iter().take(20) {
        print!("{:>5.0}", g);
    }
    println!();
    for (ci, c) in ff.c_angles().iter().enumerate() {
        print!("{:>4.0} ", c);
        for gi in 0..20 {
            print!("{:>5.2}", ff.intensity(ci, gi) / max);
        }
        println!();
    }
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut n = 0.0;
    for r in &rays {
        cx += r.kx as f64;
        cy += r.ky as f64;
        n += 1.0;
    }
    println!("mean kx {:.4} ky {:.4}", cx / n, cy / n);
    let sm = ff.smoothed(1, 1, 12.0);
    let max = sm.max_intensity();
    for g in [2.5, 45.0, 60.0, 80.0] {
        print!("g={g:>4}: ");
        let mut c = 0.0;
        while c < 360.0 {
            print!("{:>4.2}", sm.sample(c, g) / max);
            c += 5.0;
        }
        println!();
    }
}

#[allow(dead_code)]
fn unused() {}
