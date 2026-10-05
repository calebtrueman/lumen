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

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "gallery.ppm".into());
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
    let mut f = std::fs::File::create(&out).unwrap();
    write!(f, "P6\n{} {}\n255\n", cv.w, cv.h).unwrap();
    let bytes: Vec<u8> = cv.px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    f.write_all(&bytes).unwrap();
}
