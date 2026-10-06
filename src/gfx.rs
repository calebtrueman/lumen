//! Software renderer: a 32-bit `0x00RRGGBB` canvas plus anti-aliased
//! signed-distance-field painting. Everything on screen is drawn through
//! `Canvas::paint`, which makes shapes resolution independent and lets the
//! UI animate sizes smoothly.

use alloc::vec;
use alloc::vec::Vec;
use libm::{fabsf, sqrtf};

pub type Color = u32;

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[inline]
pub fn channels(c: Color) -> (f32, f32, f32) {
    (((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32)
}

#[inline]
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = (clamp01(t) * 256.0) as u32;
    let it = 256 - t;
    let rb = ((a & 0xff00ff) * it + (b & 0xff00ff) * t) >> 8;
    let g = ((a & 0x00ff00) * it + (b & 0x00ff00) * t) >> 8;
    (rb & 0xff00ff) | (g & 0x00ff00)
}

#[inline]
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![0; w * h] }
    }

    /// Blend `c` over pixel (x, y) with coverage `a` in 0..=256.
    #[inline]
    fn blend(&mut self, idx: usize, c: Color, a: u32) {
        if a >= 256 {
            self.px[idx] = c;
            return;
        }
        let d = self.px[idx];
        let ia = 256 - a;
        let rb = ((c & 0xff00ff) * a + (d & 0xff00ff) * ia) >> 8;
        let g = ((c & 0x00ff00) * a + (d & 0x00ff00) * ia) >> 8;
        self.px[idx] = (rb & 0xff00ff) | (g & 0x00ff00);
    }

    /// Paint the region bounded by (x0, y0)-(x1, y1) where `sdf(x, y)`
    /// (signed distance in pixels, negative inside) is inside the shape.
    /// `shade` gives the colour at each pixel; `alpha` scales opacity.
    pub fn paint(
        &mut self,
        bounds: (f32, f32, f32, f32),
        alpha: f32,
        sdf: impl Fn(f32, f32) -> f32,
        shade: impl Fn(f32, f32) -> Color,
    ) {
        if alpha <= 0.002 {
            return;
        }
        let (x0, y0, x1, y1) = self.clip(bounds);
        let a = clamp01(alpha) * 256.0;
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let row = y * self.w;
            for x in x0..x1 {
                let fx = x as f32 + 0.5;
                let cov = clamp01(0.5 - sdf(fx, fy));
                if cov > 0.0 {
                    self.blend(row + x, shade(fx, fy), (cov * a) as u32);
                }
            }
        }
    }

    /// Like `paint`, but `shade` also returns a per-pixel opacity, for
    /// gradients that fade out (sheens, highlights).
    pub fn paint_fading(
        &mut self,
        bounds: (f32, f32, f32, f32),
        sdf: impl Fn(f32, f32) -> f32,
        shade: impl Fn(f32, f32) -> (Color, f32),
    ) {
        let (x0, y0, x1, y1) = self.clip(bounds);
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let row = y * self.w;
            for x in x0..x1 {
                let fx = x as f32 + 0.5;
                let cov = clamp01(0.5 - sdf(fx, fy));
                if cov > 0.0 {
                    let (c, a) = shade(fx, fy);
                    self.blend(row + x, c, (cov * clamp01(a) * 256.0) as u32);
                }
            }
        }
    }

    /// Like `paint` but the closure returns a coverage in 0..1 directly,
    /// which suits soft effects (shadows, glows).
    pub fn paint_soft(
        &mut self,
        bounds: (f32, f32, f32, f32),
        color: Color,
        coverage: impl Fn(f32, f32) -> f32,
    ) {
        let (x0, y0, x1, y1) = self.clip(bounds);
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let row = y * self.w;
            for x in x0..x1 {
                let cov = coverage(x as f32 + 0.5, fy);
                if cov > 0.004 {
                    self.blend(row + x, color, (clamp01(cov) * 256.0) as u32);
                }
            }
        }
    }

    fn clip(&self, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> (usize, usize, usize, usize) {
        let cx = |v: f32| (v.max(0.0) as usize).min(self.w);
        let cy = |v: f32| (v.max(0.0) as usize).min(self.h);
        (cx(libm::floorf(x0)), cy(libm::floorf(y0)), cx(libm::ceilf(x1) + 1.0), cy(libm::ceilf(y1) + 1.0))
    }

    /// Alpha-blend an 8-bit coverage bitmap (a rasterised glyph).
    pub fn blit_mask(&mut self, x: i32, y: i32, w: usize, h: usize, mask: &[u8], c: Color, alpha: f32) {
        let a = (clamp01(alpha) * 256.0) as u32;
        for row in 0..h {
            let py = y + row as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for col in 0..w {
                let px = x + col as i32;
                if px < 0 || px >= self.w as i32 {
                    continue;
                }
                let m = mask[row * w + col] as u32;
                if m != 0 {
                    let idx = py as usize * self.w + px as usize;
                    self.blend(idx, c, (m * a) >> 8);
                }
            }
        }
    }

    /// Darken the whole canvas towards black (used for fades).
    pub fn fade(&mut self, brightness: f32) {
        let k = (clamp01(brightness) * 256.0) as u32;
        for p in self.px.iter_mut() {
            let rb = ((*p & 0xff00ff) * k) >> 8;
            let g = ((*p & 0x00ff00) * k) >> 8;
            *p = (rb & 0xff00ff) | (g & 0x00ff00);
        }
    }

    // ---- convenience shapes -------------------------------------------

    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color, alpha: f32) {
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.paint((x, y, x + w, y + h), alpha, |px, py| sd_rrect(px - cx, py - cy, hw, hh, r), |_, _| c);
    }

    pub fn rrect_gradient(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, top: Color, bottom: Color, alpha: f32) {
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.paint(
            (x, y, x + w, y + h),
            alpha,
            |px, py| sd_rrect(px - cx, py - cy, hw, hh, r),
            |_, py| mix(top, bottom, (py - y) / h),
        );
    }

    /// A 1-pixel-ish outline of a rounded rectangle.
    pub fn rrect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: Color, alpha: f32) {
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.paint(
            (x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0),
            alpha,
            |px, py| fabsf(sd_rrect(px - cx, py - cy, hw, hh, r) + width / 2.0) - width / 2.0,
            |_, _| c,
        );
    }

    /// Soft drop shadow / glow for a rounded rect.
    pub fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, blur: f32, c: Color, alpha: f32) {
        if alpha <= 0.002 {
            return;
        }
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.paint_soft((x - blur, y - blur, x + w + blur, y + h + blur), c, |px, py| {
            let d = sd_rrect(px - cx, py - cy, hw, hh, r);
            let t = 1.0 - smoothstep(-blur * 0.5, blur, d);
            t * t * alpha
        });
    }

    /// Large radial glow, falloff is quadratic for a soft, light-like look.
    pub fn glow(&mut self, cx: f32, cy: f32, radius: f32, c: Color, alpha: f32) {
        if alpha <= 0.002 {
            return;
        }
        self.paint_soft((cx - radius, cy - radius, cx + radius, cy + radius), c, |px, py| {
            let d = len(px - cx, py - cy) / radius;
            let t = clamp01(1.0 - d);
            t * t * alpha
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Color, alpha: f32) {
        self.paint((cx - r, cy - r, cx + r, cy + r), alpha, |px, py| len(px - cx, py - cy) - r, |_, _| c);
    }
}

// ---- SDF helpers ---------------------------------------------------------

#[inline]
pub fn len(x: f32, y: f32) -> f32 {
    sqrtf(x * x + y * y)
}

/// Rounded box centred at the origin with half extents (hw, hh).
#[inline]
pub fn sd_rrect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let r = r.min(hw).min(hh);
    let qx = fabsf(px) - hw + r;
    let qy = fabsf(py) - hh + r;
    len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Capsule (thick line segment) from a to b with radius r.
#[inline]
pub fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let (pax, pay, bax, bay) = (px - ax, py - ay, bx - ax, by - ay);
    let h = clamp01((pax * bax + pay * bay) / (bax * bax + bay * bay));
    len(pax - bax * h, pay - bay * h) - r
}

/// Ring of radius `r` and thickness `t` with a gap of half-angle `gap`
/// (radians) centred on direction `dir` (radians, 0 = +x, y grows down).
pub fn sd_arc(px: f32, py: f32, r: f32, t: f32, dir: f32, gap: f32) -> f32 {
    let a = libm::atan2f(py, px);
    let mut da = a - dir;
    while da > core::f32::consts::PI {
        da -= 2.0 * core::f32::consts::PI;
    }
    while da < -core::f32::consts::PI {
        da += 2.0 * core::f32::consts::PI;
    }
    if fabsf(da) < gap {
        // Distance to the nearer rounded end-cap.
        let e1 = dir + gap;
        let e2 = dir - gap;
        let d1 = len(px - r * libm::cosf(e1), py - r * libm::sinf(e1));
        let d2 = len(px - r * libm::cosf(e2), py - r * libm::sinf(e2));
        d1.min(d2) - t / 2.0
    } else {
        fabsf(len(px, py) - r) - t / 2.0
    }
}

/// Background: deep gradient with soft aurora blobs, ordered-dithered so
/// that the gradients don't band on 8-bit panels.
pub fn render_backdrop(cv: &mut Canvas) {
    const BAYER: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    let (w, h) = (cv.w as f32, cv.h as f32);
    let m = w.max(h);
    // (x, y, radius, colour, strength) in normalised coordinates.
    let blobs: [(f32, f32, f32, Color, f32); 4] = [
        (0.18, 0.12, 0.55, rgb(64, 56, 168), 0.55),
        (0.88, 0.22, 0.50, rgb(20, 110, 160), 0.45),
        (0.62, 1.05, 0.65, rgb(120, 40, 140), 0.40),
        (0.05, 0.95, 0.40, rgb(20, 70, 110), 0.30),
    ];
    let top = channels(rgb(13, 15, 30));
    let bottom = channels(rgb(6, 7, 14));
    for y in 0..cv.h {
        let ty = y as f32 / h;
        for x in 0..cv.w {
            let mut r = top.0 + (bottom.0 - top.0) * ty;
            let mut g = top.1 + (bottom.1 - top.1) * ty;
            let mut b = top.2 + (bottom.2 - top.2) * ty;
            for &(bx, by, br, bc, bs) in &blobs {
                let dx = (x as f32 - bx * w) / (br * m);
                let dy = (y as f32 - by * h) / (br * m);
                let f = 1.0 / (1.0 + 6.0 * (dx * dx + dy * dy));
                let f = f * f * bs;
                let (cr, cg, cb) = channels(bc);
                r += cr * f;
                g += cg * f;
                b += cb * f;
            }
            // Vignette.
            let vx = x as f32 / w - 0.5;
            let vy = y as f32 / h - 0.5;
            let v = 1.0 - 0.55 * (vx * vx + vy * vy);
            let d = BAYER[y & 3][x & 3] / 16.0 - 0.5;
            let q = |c: f32| (c * v + d).clamp(0.0, 255.0) as u32;
            cv.px[y * cv.w + x] = (q(r) << 16) | (q(g) << 8) | q(b);
        }
    }
}

/// Signed distance to a closed polygon (negative inside).
pub fn sd_polygon(px: f32, py: f32, v: &[(f32, f32)]) -> f32 {
    let dot = |ax: f32, ay: f32, bx: f32, by: f32| ax * bx + ay * by;
    let mut d = dot(px - v[0].0, py - v[0].1, px - v[0].0, py - v[0].1);
    let mut s = 1.0;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let (ex, ey) = (v[j].0 - v[i].0, v[j].1 - v[i].1);
        let (wx, wy) = (px - v[i].0, py - v[i].1);
        let t = clamp01(dot(wx, wy, ex, ey) / dot(ex, ey, ex, ey));
        let (bx, by) = (wx - ex * t, wy - ey * t);
        d = d.min(dot(bx, by, bx, by));
        let c1 = py >= v[i].1;
        let c2 = py < v[j].1;
        let c3 = ex * wy > ey * wx;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    s * sqrtf(d)
}

/// Classic arrow pointer with a dark outline and a soft shadow; (x, y) is the tip.
pub fn draw_cursor(cv: &mut Canvas, x: f32, y: f32, size: f32) {
    let u = size / 20.0;
    let pts: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 16.5), (4.0, 12.8), (6.8, 19.2), (9.6, 18.0), (6.9, 11.8), (12.2, 11.8)];
    let v: [(f32, f32); 7] = core::array::from_fn(|i| (x + pts[i].0 * u, y + pts[i].1 * u));
    let b = (x - 3.0 * u, y - 3.0 * u, x + 16.0 * u, y + 23.0 * u);
    let sh: [(f32, f32); 7] = core::array::from_fn(|i| (v[i].0 + 0.8 * u, v[i].1 + 1.6 * u));
    cv.paint_soft((b.0, b.1, b.2 + 4.0 * u, b.3 + 4.0 * u), rgb(0, 0, 0), |px, py| {
        0.35 * (1.0 - smoothstep(-1.0 * u, 3.0 * u, sd_polygon(px, py, &sh)))
    });
    let line = 1.3 * u.max(0.8);
    cv.paint(b, 1.0, |px, py| sd_polygon(px, py, &v) - line, |_, _| rgb(20, 20, 26));
    cv.paint(b, 1.0, |px, py| sd_polygon(px, py, &v), |_, _| rgb(255, 255, 255));
}

/// A full-colour image: premultiplied RGBA, row-major.
#[derive(Debug, PartialEq)]
pub struct Sprite {
    pub w: usize,
    pub h: usize,
    pub rgba: &'static [u8],
}

impl Canvas {
    /// Draw `img` scaled into the box (x, y, w, h). Each target pixel
    /// averages the source pixels it covers (area filter), so large
    /// reductions stay smooth instead of aliasing.
    pub fn image(&mut self, img: &Sprite, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        let a = clamp01(alpha);
        if a <= 0.002 || w < 1.0 || h < 1.0 {
            return;
        }
        let (x0, y0, x1, y1) = self.clip((x, y, x + w - 1.0, y + h - 1.0));
        let (sx, sy) = (img.w as f32 / w, img.h as f32 / h);
        for py in y0..y1 {
            let fy0 = ((py as f32 - y) * sy).max(0.0);
            let fy1 = ((py as f32 + 1.0 - y) * sy).min(img.h as f32);
            if fy1 <= fy0 {
                continue;
            }
            let (ry0, ry1) = (fy0 as usize, (libm::ceilf(fy1) as usize).min(img.h));
            for px in x0..x1 {
                let fx0 = ((px as f32 - x) * sx).max(0.0);
                let fx1 = ((px as f32 + 1.0 - x) * sx).min(img.w as f32);
                if fx1 <= fx0 {
                    continue;
                }
                let (rx0, rx1) = (fx0 as usize, (libm::ceilf(fx1) as usize).min(img.w));
                let mut acc = [0.0f32; 4];
                let mut wsum = 0.0;
                for iy in ry0..ry1 {
                    let wy = (fy1.min(iy as f32 + 1.0) - fy0.max(iy as f32)).max(0.0);
                    for ix in rx0..rx1 {
                        let wx = (fx1.min(ix as f32 + 1.0) - fx0.max(ix as f32)).max(0.0);
                        let wgt = wx * wy;
                        let i = (iy * img.w + ix) * 4;
                        for c in 0..4 {
                            acc[c] += img.rgba[i + c] as f32 * wgt;
                        }
                        wsum += wgt;
                    }
                }
                if wsum <= 0.0 {
                    continue;
                }
                let sa = acc[3] / wsum / 255.0 * a;
                if sa <= 0.002 {
                    continue;
                }
                // Premultiplied "over": dst = src + dst * (1 - src_alpha).
                let idx = py * self.w + px;
                let (dr, dg, db) = channels(self.px[idx]);
                let k = 1.0 - sa;
                let ch = |c: usize, d: f32| (acc[c] / wsum * a + d * k).clamp(0.0, 255.0) as u32;
                self.px[idx] = (ch(0, dr) << 16) | (ch(1, dg) << 8) | ch(2, db);
            }
        }
    }
}
