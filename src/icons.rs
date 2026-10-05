//! Vector OS icons drawn as signed distance fields, so they stay crisp at
//! any size and can be scaled smoothly during animations.

use crate::gfx::*;
use crate::text::{Face, Text};
use core::f32::consts::PI;
use libm::{atan2f, cosf, fabsf, sinf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Glyph {
    Windows,
    Tux,
    Gear,
    Drive,
    Shell,
    /// A glyph from the Font Logos distro font.
    Logo(char),
    /// A glyph from the Font Awesome brands font.
    Brand(char),
    Letter(char),
}

/// An app-icon style tile: a gradient squircle with a glyph on it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Icon {
    pub glyph: Glyph,
    pub top: Color,
    pub bottom: Color,
    pub ink: Color,
}

impl Icon {
    /// Colour used for the glow behind the selected card.
    pub fn accent(self) -> Color {
        mix(self.top, self.bottom, 0.4)
    }
}

/// Draw an app-icon style tile with the OS glyph, centred at (cx, cy).
pub fn draw_tile(cv: &mut Canvas, text: &mut Text, icon: Icon, cx: f32, cy: f32, size: f32, alpha: f32) {
    let (top, bottom) = (icon.top, icon.bottom);
    let (x, y) = (cx - size / 2.0, cy - size / 2.0);
    let r = size * 0.24;
    cv.shadow(x, y + size * 0.06, size, size, r, size * 0.16, rgb(0, 0, 0), 0.45 * alpha);
    cv.rrect_gradient(x, y, size, size, r, top, bottom, alpha);
    // Glossy top sheen.
    let (hw, hh) = (size / 2.0, size / 2.0);
    cv.paint_fading(
        (x, y, x + size, y + size * 0.6),
        |px, py| sd_rrect(px - cx, py - cy, hw, hh, r),
        |_, py| {
            let t = clamp01((py - y) / (size * 0.6));
            (rgb(255, 255, 255), alpha * 0.26 * (1.0 - t) * (1.0 - t))
        },
    );
    cv.rrect_stroke(x, y, size, size, r, 1.0, rgb(255, 255, 255), 0.18 * alpha);
    draw_glyph(cv, text, icon, cx, cy, size, alpha);
}

fn draw_glyph(cv: &mut Canvas, text: &mut Text, icon: Icon, cx: f32, cy: f32, u: f32, alpha: f32) {
    let ink = icon.ink;
    let bounds = (cx - u / 2.0, cy - u / 2.0, cx + u / 2.0, cy + u / 2.0);
    match icon.glyph {
        Glyph::Windows => {
            let g = u * 0.46; // logo extent
            let gap = u * 0.035;
            let q = (g - gap) / 2.0;
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let qx = cx + sx * (gap / 2.0 + q / 2.0);
                let qy = cy + sy * (gap / 2.0 + q / 2.0);
                cv.paint(bounds, alpha, |px, py| sd_rrect(px - qx, py - qy, q / 2.0, q / 2.0, u * 0.012), |_, _| ink);
            }
        }
        Glyph::Tux => draw_tux(cv, cx, cy + u * 0.02, u * 0.95, alpha),
        Glyph::Logo(c) => text.draw_glyph_centered(cv, Face::Logos, c, u * 0.56, cx, cy, ink, alpha),
        Glyph::Brand(c) => text.draw_glyph_centered(cv, Face::Brands, c, u * 0.52, cx, cy, ink, alpha),
        Glyph::Letter(c) => text.draw_glyph_centered(cv, Face::Display, c, u * 0.6, cx, cy, ink, alpha),
        Glyph::Gear => draw_gear(cv, cx, cy, u * 0.6, ink, alpha),
        Glyph::Drive => {
            let (w, h) = (u * 0.5, u * 0.3);
            cv.paint(
                bounds,
                alpha,
                |px, py| fabsf(sd_rrect(px - cx, py - cy, w / 2.0, h / 2.0, u * 0.05)) - u * 0.022,
                |_, _| ink,
            );
            cv.circle(cx + w * 0.28, cy, u * 0.035, ink, alpha);
            cv.paint(bounds, alpha, |px, py| sd_segment(px, py, cx - w * 0.3, cy, cx, cy, u * 0.018), |_, _| ink);
        }
        Glyph::Shell => {
            let green = rgb(120, 230, 160);
            let t = u * 0.025;
            let (ox, oy) = (cx - u * 0.18, cy);
            cv.paint(
                bounds,
                alpha,
                |px, py| {
                    sd_segment(px, py, ox, oy - u * 0.1, ox + u * 0.1, oy, t)
                        .min(sd_segment(px, py, ox + u * 0.1, oy, ox, oy + u * 0.1, t))
                },
                |_, _| green,
            );
            cv.paint(bounds, alpha, |px, py| sd_segment(px, py, cx, cy + u * 0.1, cx + u * 0.18, cy + u * 0.1, t), |_, _| ink);
        }
    }
}

fn draw_tux(cv: &mut Canvas, cx: f32, cy: f32, u: f32, alpha: f32) {
    let black = rgb(24, 24, 28);
    let belly = rgb(250, 250, 248);
    let orange = rgb(250, 160, 30);
    let b = (cx - u / 2.0, cy - u / 2.0, cx + u / 2.0, cy + u / 2.0);
    // Feet
    for s in [-1.0, 1.0] {
        let fx = cx + s * u * 0.095;
        cv.paint(b, alpha, |px, py| sd_ellipse(px - fx, py - (cy + u * 0.29), u * 0.095, u * 0.042), |_, _| orange);
    }
    // Body + head
    cv.paint(
        b,
        alpha,
        |px, py| {
            let body = sd_ellipse(px - cx, py - (cy + u * 0.05), u * 0.19, u * 0.25);
            let head = len(px - cx, py - (cy - u * 0.16)) - u * 0.13;
            smin(body, head, u * 0.06)
        },
        |_, _| black,
    );
    // Belly
    cv.paint(b, alpha, |px, py| sd_ellipse(px - cx, py - (cy + u * 0.1), u * 0.125, u * 0.18), |_, _| belly);
    // Eyes
    for s in [-1.0, 1.0] {
        let ex = cx + s * u * 0.048;
        let ey = cy - u * 0.185;
        cv.paint(b, alpha, |px, py| sd_ellipse(px - ex, py - ey, u * 0.033, u * 0.042), |_, _| belly);
        cv.circle(ex + s * u * 0.01, ey + u * 0.008, u * 0.018, black, alpha);
    }
    // Beak
    cv.paint(b, alpha, |px, py| sd_ellipse(px - cx, py - (cy - u * 0.118), u * 0.062, u * 0.032), |_, _| orange);
}

pub fn draw_gear(cv: &mut Canvas, cx: f32, cy: f32, d: f32, c: Color, alpha: f32) {
    let r = d / 2.0;
    cv.paint((cx - r, cy - r, cx + r, cy + r), alpha, |px, py| {
        let (x, y) = (px - cx, py - cy);
        let a = atan2f(y, x);
        let teeth = (cosf(a * 8.0) * 2.2).clamp(-1.0, 1.0);
        let outer = len(x, y) - (r * 0.78 + r * 0.14 * teeth);
        outer.max(-(len(x, y) - r * 0.32))
    }, |_, _| c);
}

pub fn draw_power(cv: &mut Canvas, cx: f32, cy: f32, d: f32, c: Color, alpha: f32) {
    let r = d / 2.0;
    let t = d * 0.11;
    cv.paint((cx - r, cy - r, cx + r, cy + r), alpha, |px, py| {
        let arc = sd_arc(px - cx, py - cy, r * 0.72, t, -PI / 2.0, 0.65);
        let bar = sd_segment(px, py, cx, cy - r * 0.9, cx, cy - r * 0.1, t / 2.0);
        arc.min(bar)
    }, |_, _| c);
}

pub fn draw_restart(cv: &mut Canvas, cx: f32, cy: f32, d: f32, c: Color, alpha: f32) {
    let r = d / 2.0;
    let t = d * 0.11;
    let ar = r * 0.72;
    // Arc open at the top-right, with an arrow head at its upper end.
    let end = -PI / 2.0 + 0.1;
    let (ex, ey) = (cx + ar * cosf(end), cy + ar * sinf(end));
    cv.paint((cx - r, cy - r, cx + r, cy + r), alpha, |px, py| {
        let arc = sd_arc(px - cx, py - cy, ar, t, -PI / 4.0, 0.68);
        let h = r * 0.38;
        let a1 = sd_segment(px, py, ex, ey, ex - h * 0.9, ey - h * 0.55, t / 2.0);
        let a2 = sd_segment(px, py, ex, ey, ex - h * 0.75, ey + h * 0.75, t / 2.0);
        arc.min(a1).min(a2)
    }, |_, _| c);
}

/// Stopwatch-style timer: a ring with a hand and a button on top.
pub fn draw_timer(cv: &mut Canvas, cx: f32, cy: f32, d: f32, c: Color, alpha: f32) {
    let r = d / 2.0;
    let t = d * 0.1;
    let (ccx, ccy) = (cx, cy + r * 0.08);
    let ring = r * 0.72;
    cv.paint((cx - r, cy - r, cx + r, cy + r), alpha, |px, py| {
        let face = fabsf(len(px - ccx, py - ccy) - ring) - t / 2.0;
        let hand = sd_segment(px, py, ccx, ccy, ccx + ring * 0.42, ccy - ring * 0.42, t / 2.0);
        let button = sd_segment(px, py, ccx, ccy - ring - t * 1.2, ccx, ccy - ring - t * 0.2, t / 2.0);
        face.min(hand).min(button)
    }, |_, _| c);
}
