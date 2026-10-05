//! Host-side gallery of every OS tile, built from Lumen's own source files.

#![allow(dead_code)]

extern crate alloc;

#[path = "../../../src/gfx.rs"]
mod gfx;
#[path = "../../../src/icons.rs"]
mod icons;
#[path = "../../../src/os.rs"]
mod os;
#[path = "../../../src/text.rs"]
mod text;

use std::io::Write;

/// Lumen's own app icon: indigo tile with a four-point sparkle.
fn render_icon(size: usize, bg: u32) -> gfx::Canvas {
    let mut cv = gfx::Canvas::new(size, size);
    cv.px.fill(bg);
    let s = size as f32;
    let pad = s * 0.06;
    let (x, y, w) = (pad, pad, s - 2.0 * pad);
    let r = w * 0.24;
    cv.rrect_gradient(x, y, w, w, r, gfx::rgb(118, 108, 255), gfx::rgb(58, 40, 190), 1.0);
    let (hw, cx, cy) = (w / 2.0, s / 2.0, s / 2.0);
    cv.paint_fading((x, y, x + w, y + w * 0.6), |px, py| gfx::sd_rrect(px - cx, py - cy, hw, hw, r), |_, py| {
        let t = gfx::clamp01((py - y) / (w * 0.6));
        (gfx::rgb(255, 255, 255), 0.28 * (1.0 - t) * (1.0 - t))
    });
    let star = |cx: f32, cy: f32, ro: f32| -> [(f32, f32); 8] {
        core::array::from_fn(|i| {
            let a = i as f32 * core::f32::consts::FRAC_PI_4 - core::f32::consts::FRAC_PI_2;
            let rr = if i % 2 == 0 { ro } else { ro * 0.26 };
            (cx + rr * libm::cosf(a), cy + rr * libm::sinf(a))
        })
    };
    let big = star(cx - w * 0.04, cy + w * 0.04, w * 0.33);
    let small = star(cx + w * 0.22, cy - w * 0.22, w * 0.12);
    let white = gfx::rgb(255, 255, 255);
    cv.paint((0.0, 0.0, s, s), 1.0, |px, py| gfx::sd_polygon(px, py, &big) - w * 0.012, |_, _| white);
    cv.paint((0.0, 0.0, s, s), 0.9, |px, py| gfx::sd_polygon(px, py, &small) - w * 0.006, |_, _| white);
    cv
}

fn write_ppm(path: &str, cv: &gfx::Canvas) {
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "P6\n{} {}\n255\n", cv.w, cv.h).unwrap();
    let bytes: Vec<u8> = cv.px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    f.write_all(&bytes).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `--icon DIR`: write icon-<size>-{black,white}.ppm for alpha recovery.
    if args.get(1).map(String::as_str) == Some("--icon") {
        for size in [16, 24, 32, 48, 64, 128, 256] {
            for (name, bg) in [("black", 0x000000), ("white", 0xffffff)] {
                write_ppm(&format!("{}/icon-{size}-{name}.ppm", args[2]), &render_icon(size, bg));
            }
        }
        return;
    }
    // `--bench W H`: time a Lumen-like menu frame (backdrop copy, glow, four
    // cards with icons and text, power row) at that resolution.
    if args.get(1).map(String::as_str) == Some("--bench") {
        let (w, h): (usize, usize) = (args[2].parse().unwrap(), args[3].parse().unwrap());
        let mut bg = gfx::Canvas::new(w, h);
        let t = std::time::Instant::now();
        gfx::render_backdrop(&mut bg);
        println!("backdrop (once):      {:6.1} ms", t.elapsed().as_secs_f64() * 1e3);
        let mut cv = gfx::Canvas::new(w, h);
        let mut text = text::Text::new();
        let s = (h as f32 / 1080.0).clamp(0.55, 2.5).min(w as f32 / 1280.0 * 1.2);
        let (cw, ch, gap) = (212.0 * s, 236.0 * s, 34.0 * s);
        let icons = [os::WINDOWS, os::by_name("Fedora").unwrap().icon, os::by_name("Ubuntu").unwrap().icon, os::by_name("Bazzite").unwrap().icon];
        let mut phases = [0.0f64; 5];
        let frames = 30;
        for f in 0..frames {
            let t0 = std::time::Instant::now();
            cv.px.copy_from_slice(&bg.px);
            let t1 = std::time::Instant::now();
            let total = 4.0 * cw + 3.0 * gap;
            let x0 = (w as f32 - total) / 2.0;
            let row_y = h as f32 * 0.47;
            let sel = f % 4;
            let cx = |i: usize| x0 + i as f32 * (cw + gap) + cw / 2.0;
            cv.glow(cx(sel), row_y, cw * 1.7, icons[sel].accent(), 0.6);
            let t2 = std::time::Instant::now();
            for (i, icon) in icons.iter().enumerate() {
                let sc = if i == sel { 1.07 } else { 1.0 };
                let (cww, chh) = (cw * sc, ch * sc);
                let (x, y) = (cx(i) - cww / 2.0, row_y - chh / 2.0);
                cv.shadow(x, y + 16.0 * s, cww, chh, 28.0 * s, 40.0 * s, gfx::rgb(0, 0, 0), 0.32);
                cv.rrect(x, y, cww, chh, 28.0 * s, gfx::rgb(255, 255, 255), 0.08);
                cv.rrect_stroke(x, y, cww, chh, 28.0 * s, 1.2 * s, gfx::rgb(255, 255, 255), 0.2);
                icons::draw_tile(&mut cv, &mut text, *icon, cx(i), y + chh * 0.43, 116.0 * s * sc, 1.0);
            }
            let t3 = std::time::Instant::now();
            for i in 0..4 {
                text.draw_centered(&mut cv, text::Face::Body, 19.0 * s, "Windows", cx(i), row_y + ch * 0.4, gfx::rgb(255, 255, 255), 0.9);
            }
            text.draw(&mut cv, text::Face::Display, 34.0 * s, "23:40", 64.0 * s, 82.0 * s, gfx::rgb(255, 255, 255), 0.9);
            for i in 0..3 {
                let x = w as f32 / 2.0 - 300.0 * s + i as f32 * 200.0 * s;
                cv.rrect(x, h as f32 - 110.0 * s, 180.0 * s, 46.0 * s, 23.0 * s, gfx::rgb(255, 255, 255), 0.06);
            }
            let t4 = std::time::Instant::now();
            for (k, (a, b)) in [(t0, t1), (t1, t2), (t2, t3), (t3, t4), (t0, t4)].iter().enumerate() {
                phases[k] += (*b - *a).as_secs_f64() * 1e3;
            }
        }
        let n = frames as f64;
        println!("{w}x{h}, per frame (avg of {frames}):");
        for (k, name) in ["copy background", "selection glow", "4 cards + icons", "text + buttons", "TOTAL draw"].iter().enumerate() {
            println!("  {name:18} {:6.1} ms", phases[k] / n);
        }
        let mb = (w * h * 4) as f64 / 1e6;
        println!("  full-frame present = {mb:.1} MB per frame");
        return;
    }
    let out = args.get(1).cloned().unwrap_or_else(|| "gallery.ppm".into());
    let cols = 10;
    let (cell_w, cell_h) = (150.0, 170.0);
    let rows = os::ALL.len().div_ceil(cols);
    let mut cv = gfx::Canvas::new((cols as f32 * cell_w) as usize, (rows as f32 * cell_h + 20.0) as usize);
    gfx::render_backdrop(&mut cv);
    let mut text = text::Text::new();
    for (i, o) in os::ALL.iter().enumerate() {
        let cx = (i % cols) as f32 * cell_w + cell_w / 2.0;
        let cy = (i / cols) as f32 * cell_h + 80.0;
        icons::draw_tile(&mut cv, &mut text, o.icon, cx, cy, 96.0, 1.0);
        let label = text.fit(text::Face::Body, 13.0, o.name, cell_w - 10.0);
        text.draw_centered(&mut cv, text::Face::Body, 13.0, &label, cx, cy + 72.0, gfx::rgb(255, 255, 255), 0.85);
    }
    write_ppm(&out, &cv);
}
