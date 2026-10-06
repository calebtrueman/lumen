//! TrueType text via fontdue, with a per-(glyph, size) bitmap cache.

use crate::gfx::{Canvas, Color};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use fontdue::{Font, FontSettings, Metrics};

static BODY_TTF: &[u8] = include_bytes!("../../../assets/Inter-Medium.ttf");
static DISPLAY_TTF: &[u8] = include_bytes!("../../../assets/InterDisplay-SemiBold.ttf");
static LOGOS_TTF: &[u8] = include_bytes!("../../../assets/logos.ttf");
static BRANDS_OTF: &[u8] = include_bytes!("../../../assets/brands.otf");

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Face {
    Body,
    Display,
    Logos,
    Brands,
}

struct Glyph {
    m: Metrics,
    bmp: Vec<u8>,
}

pub struct Text {
    body: Font,
    display: Font,
    logos: Font,
    brands: Font,
    cache: BTreeMap<(Face, u32, char), Glyph>,
}

impl Text {
    pub fn new() -> Self {
        let load = |data: &'static [u8]| Font::from_bytes(data, FontSettings::default()).expect("embedded font is valid");
        Self {
            body: load(BODY_TTF),
            display: load(DISPLAY_TTF),
            logos: load(LOGOS_TTF),
            brands: load(BRANDS_OTF),
            cache: BTreeMap::new(),
        }
    }

    fn font(&self, face: Face) -> &Font {
        match face {
            Face::Body => &self.body,
            Face::Display => &self.display,
            Face::Logos => &self.logos,
            Face::Brands => &self.brands,
        }
    }

    /// Whether the font can render `c` (used to choose glyph fallbacks).
    pub fn has(&self, c: char) -> bool {
        self.body.lookup_glyph_index(c) != 0
    }

    fn glyph(&mut self, face: Face, size: u32, c: char) -> &Glyph {
        if !self.cache.contains_key(&(face, size, c)) {
            let (m, bmp) = self.font(face).rasterize(c, size as f32);
            self.cache.insert((face, size, c), Glyph { m, bmp });
        }
        &self.cache[&(face, size, c)]
    }

    pub fn width(&mut self, face: Face, size: f32, s: &str) -> f32 {
        let size = libm::roundf(size).max(1.0) as u32;
        let mut w = 0.0;
        let mut prev = None;
        for c in s.chars() {
            if let Some(p) = prev {
                w += self.font(face).horizontal_kern(p, c, size as f32).unwrap_or(0.0);
            }
            w += self.glyph(face, size, c).m.advance_width;
            prev = Some(c);
        }
        w
    }

    /// Draw `s` with its baseline at `y`, starting at `x`.
    pub fn draw(&mut self, cv: &mut Canvas, face: Face, size: f32, s: &str, x: f32, y: f32, c: Color, alpha: f32) {
        let size = libm::roundf(size).max(1.0) as u32;
        let mut pen = x;
        let mut prev = None;
        for ch in s.chars() {
            if let Some(p) = prev {
                pen += self.font(face).horizontal_kern(p, ch, size as f32).unwrap_or(0.0);
            }
            let g = self.glyph(face, size, ch);
            let gx = libm::roundf(pen + g.m.xmin as f32) as i32;
            let gy = libm::roundf(y - g.m.ymin as f32 - g.m.height as f32) as i32;
            cv.blit_mask(gx, gy, g.m.width, g.m.height, &g.bmp, c, alpha);
            pen += g.m.advance_width;
            prev = Some(ch);
        }
    }

    /// Draw a single glyph centred on (cx, cy) by its ink bounds, which
    /// suits logos whose baseline metrics are arbitrary.
    pub fn draw_glyph_centered(&mut self, cv: &mut Canvas, face: Face, c: char, size: f32, cx: f32, cy: f32, color: Color, alpha: f32) {
        let size = libm::roundf(size).max(1.0) as u32;
        let g = self.glyph(face, size, c);
        let x = libm::roundf(cx - g.m.width as f32 / 2.0) as i32;
        let y = libm::roundf(cy - g.m.height as f32 / 2.0) as i32;
        cv.blit_mask(x, y, g.m.width, g.m.height, &g.bmp, color, alpha);
    }

    pub fn draw_centered(&mut self, cv: &mut Canvas, face: Face, size: f32, s: &str, cx: f32, y: f32, c: Color, alpha: f32) {
        let w = self.width(face, size, s);
        self.draw(cv, face, size, s, cx - w / 2.0, y, c, alpha);
    }

    /// Shorten `s` with an ellipsis so it fits within `max` pixels.
    pub fn fit(&mut self, face: Face, size: f32, s: &str, max: f32) -> alloc::string::String {
        if self.width(face, size, s) <= max {
            return s.into();
        }
        let mut out: alloc::string::String = s.into();
        while !out.is_empty() {
            out.pop();
            let candidate = alloc::format!("{}…", out.trim_end());
            if self.width(face, size, &candidate) <= max {
                return candidate;
            }
        }
        "…".into()
    }
}
