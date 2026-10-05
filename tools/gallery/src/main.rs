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
