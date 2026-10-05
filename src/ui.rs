//! Picker state, layout, animation and input.

use crate::discover::Entry;
use crate::gfx::*;
use crate::icons;
use crate::text::{Face, Text};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libm::expf;
use uefi::proto::console::text::{Key, ScanCode};

#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Firmware,
    Restart,
    Shutdown,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Action::Firmware => "Firmware Settings",
            Action::Restart => "Restart",
            Action::Shutdown => "Shut Down",
        }
    }
}

pub enum Command {
    None,
    Boot(usize),
    Act(Action),
    Rescan,
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Card(usize),
    Action(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Entries,
    Actions,
}

pub struct Ui {
    pub entries: Vec<Entry>,
    actions: Vec<Action>,
    sel: usize,
    act_sel: usize,
    row: Row,
    focus: Vec<f32>,
    act_focus: Vec<f32>,
    cam: f32,
    countdown: Option<(f64, f64)>, // (start, length)
    toast: Option<(String, f64, bool)>, // (message, shown at, is error)
    /// When each card started its entrance animation.
    appear: Vec<f64>,
    pub show_clock: bool,
    clock: (String, String, f64),
    redraw: bool,
    /// Clickable regions from the last frame: (x, y, w, h, target).
    hits: Vec<(f32, f32, f32, f32, Target)>,
    /// Pointer position while a mouse is in use; hidden when typing.
    cursor: Option<(f32, f32)>,
}

const ANIM_RATE: f32 = 14.0;

impl Ui {
    pub fn new(entries: Vec<Entry>, sel: usize, firmware_setup: bool, now: f64) -> Self {
        let mut actions = Vec::new();
        if firmware_setup {
            actions.push(Action::Firmware);
        }
        actions.push(Action::Restart);
        actions.push(Action::Shutdown);
        let n = entries.len();
        let act_n = actions.len();
        Self {
            sel: sel.min(n.saturating_sub(1)),
            entries,
            act_focus: alloc::vec![0.0; act_n],
            actions,
            act_sel: 0,
            row: Row::Entries,
            focus: alloc::vec![0.0; n],
            cam: f32::NAN,
            countdown: None,
            toast: None,
            appear: (0..n).map(|i| now + 0.06 * i as f64).collect(),
            show_clock: true,
            clock: (String::new(), String::new(), -10.0),
            redraw: true,
            hits: Vec::new(),
            cursor: None,
        }
    }

    /// Replace the entry list after a rescan. Cards that were already
    /// shown keep their state; only new ones animate in, and the selection
    /// stays on the same OS. Returns the titles of newly found entries.
    pub fn set_entries(&mut self, entries: Vec<Entry>, now: f64) -> Vec<String> {
        let old = |id: &str| self.entries.iter().position(|e| e.id == id);
        let selected = self.entries.get(self.sel).map(|e| e.id.clone());
        let mut fresh = Vec::new();
        let mut focus = Vec::new();
        let mut appear = Vec::new();
        for e in &entries {
            match old(&e.id) {
                Some(i) => {
                    focus.push(self.focus[i]);
                    appear.push(self.appear[i]);
                }
                None => {
                    focus.push(0.0);
                    appear.push(now + 0.06 * fresh.len() as f64);
                    fresh.push(e.title.clone());
                }
            }
        }
        self.sel = selected.and_then(|id| entries.iter().position(|e| e.id == id)).unwrap_or(0).min(entries.len().saturating_sub(1));
        self.focus = focus;
        self.appear = appear;
        self.entries = entries;
        self.redraw = true;
        fresh
    }

    pub fn selected(&self) -> usize {
        self.sel
    }

    pub fn start_countdown(&mut self, now: f64, secs: f64) {
        if !self.entries.is_empty() {
            self.countdown = Some((now, secs));
        }
    }

    /// True once the countdown has run out.
    pub fn countdown_done(&self, now: f64) -> bool {
        matches!(self.countdown, Some((s, len)) if now - s >= len)
    }

    pub fn toast(&mut self, msg: String, now: f64) {
        self.toast = Some((msg, now, true));
        self.redraw = true;
    }

    pub fn notify(&mut self, msg: String, now: f64) {
        self.toast = Some((msg, now, false));
        self.redraw = true;
    }

    /// UI scale factor for a screen of this size.
    pub fn scale(w: usize, h: usize) -> f32 {
        (h as f32 / 1080.0).clamp(0.55, 2.5).min(w as f32 / 1280.0 * 1.2)
    }

    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|&&(hx, hy, hw, hh, _)| x >= hx && x < hx + hw && y >= hy && y < hy + hh).map(|h| h.4)
    }

    /// Hovering selects what's under the pointer.
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        self.cursor = Some((x, y));
        self.countdown = None;
        self.redraw = true;
        match self.hit(x, y) {
            Some(Target::Card(i)) => {
                self.sel = i;
                self.row = Row::Entries;
            }
            Some(Target::Action(i)) => {
                self.act_sel = i;
                self.row = Row::Actions;
            }
            None => {}
        }
    }

    pub fn pointer_clicked(&mut self, x: f32, y: f32) -> Command {
        self.pointer_moved(x, y);
        match self.hit(x, y) {
            Some(Target::Card(i)) => Command::Boot(i),
            Some(Target::Action(i)) => Command::Act(self.actions[i]),
            None => Command::None,
        }
    }

    pub fn scroll(&mut self, notches: i32) {
        self.countdown = None;
        self.redraw = true;
        let n = self.entries.len() as i32;
        if n > 0 {
            self.row = Row::Entries;
            self.sel = (self.sel as i32 + notches).clamp(0, n - 1) as usize;
        }
    }

    pub fn key(&mut self, key: Key) -> Command {
        let had_countdown = self.countdown.take().is_some();
        self.cursor = None;
        self.redraw = true;
        let n = self.entries.len();
        match key {
            Key::Special(ScanCode::LEFT) => match self.row {
                Row::Entries => self.sel = self.sel.saturating_sub(1),
                Row::Actions => self.act_sel = self.act_sel.saturating_sub(1),
            },
            Key::Special(ScanCode::RIGHT) => match self.row {
                Row::Entries => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                Row::Actions => self.act_sel = (self.act_sel + 1).min(self.actions.len() - 1),
            },
            Key::Special(ScanCode::HOME) if self.row == Row::Entries => self.sel = 0,
            Key::Special(ScanCode::END) if self.row == Row::Entries => self.sel = n.saturating_sub(1),
            Key::Special(ScanCode::UP) if n > 0 => self.row = Row::Entries,
            Key::Special(ScanCode::DOWN) => self.row = Row::Actions,
            Key::Special(ScanCode::FUNCTION_5) => return Command::Rescan,
            Key::Special(ScanCode::ESCAPE) => {
                if !had_countdown {
                    self.row = Row::Entries;
                }
            }
            Key::Printable(c) => match char::from(c) {
                '\r' | '\n' | ' ' => {
                    return match self.row {
                        Row::Entries if n > 0 => Command::Boot(self.sel),
                        Row::Actions => Command::Act(self.actions[self.act_sel]),
                        _ => Command::None,
                    };
                }
                '\t' => {
                    self.row = if self.row == Row::Entries || n == 0 { Row::Actions } else { Row::Entries };
                }
                d @ '1'..='9' => {
                    let i = d as usize - '1' as usize;
                    if i < n {
                        self.sel = i;
                        self.row = Row::Entries;
                        return Command::Boot(i);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        if n == 0 {
            self.row = Row::Actions;
        }
        Command::None
    }

    /// Advance animations; returns true if a redraw is needed.
    pub fn update(&mut self, dt: f32, now: f64) -> bool {
        let k = 1.0 - expf(-dt * ANIM_RATE);
        let mut moving = core::mem::take(&mut self.redraw);
        for (i, f) in self.focus.iter_mut().enumerate() {
            let target = if i == self.sel { if self.row == Row::Entries { 1.0 } else { 0.55 } } else { 0.0 };
            moving |= step(f, target, k);
        }
        for (i, f) in self.act_focus.iter_mut().enumerate() {
            let target = if self.row == Row::Actions && i == self.act_sel { 1.0 } else { 0.0 };
            moving |= step(f, target, k);
        }
        moving |= self.appear.iter().any(|&t| now - t < 0.7);
        moving |= self.countdown.is_some();
        if let Some((_, t, _)) = self.toast {
            if now - t > 4.5 {
                self.toast = None;
            }
            moving = true;
        }
        if now - self.clock.2 >= 1.0 {
            let prev = self.clock.0.clone();
            self.clock = clock_strings(now);
            moving |= self.show_clock && prev != self.clock.0;
        }
        moving
    }

    pub fn draw(&mut self, cv: &mut Canvas, bg: &[u32], text: &mut Text, now: f64, dt: f32) {
        cv.px.copy_from_slice(bg);
        self.hits.clear();
        let (w, h) = (cv.w as f32, cv.h as f32);
        let s = Self::scale(cv.w, cv.h);
        let white = rgb(255, 255, 255);

        // ---- header ---------------------------------------------------
        let margin = 64.0 * s;
        if self.show_clock && !self.clock.0.is_empty() {
            text.draw(cv, Face::Display, 34.0 * s, &self.clock.0, margin, 82.0 * s, white, 0.92);
            text.draw(cv, Face::Body, 16.0 * s, &self.clock.1, margin, 110.0 * s, white, 0.5);
        }
        let hints = self.hint_text(text);
        let hw = text.width(Face::Body, 14.0 * s, &hints);
        text.draw(cv, Face::Body, 14.0 * s, &hints, w - margin - hw, 82.0 * s, white, 0.42);

        // ---- entry row ------------------------------------------------
        let n = self.entries.len();
        let (cw, ch, gap) = (212.0 * s, 236.0 * s, 34.0 * s);
        let row_y = h * 0.47;
        if n == 0 {
            text.draw_centered(cv, Face::Display, 30.0 * s, "No operating systems found", w / 2.0, row_y - 10.0 * s, white, 0.9);
            text.draw_centered(
                cv,
                Face::Body,
                16.0 * s,
                "Check that your drives are connected, then press F5 to look again.",
                w / 2.0,
                row_y + 26.0 * s,
                white,
                0.5,
            );
        } else {
            let total = n as f32 * cw + (n - 1) as f32 * gap;
            let avail = w - 2.0 * margin;
            let (origin, cam_target) = if total <= avail {
                ((w - total) / 2.0, 0.0)
            } else {
                let sel_c = self.sel as f32 * (cw + gap) + cw / 2.0;
                (margin, (sel_c - avail / 2.0).clamp(0.0, total - avail))
            };
            if self.cam.is_nan() {
                self.cam = cam_target;
            }
            if step(&mut self.cam, cam_target, 1.0 - expf(-dt * 10.0)) {
                self.redraw = true;
            }
            let layout: Vec<(f32, f32, f32, f32)> = (0..n)
                .map(|i| {
                    let p = ease_out(clamp01(((now - self.appear[i]) / 0.55) as f32));
                    let cx = origin + i as f32 * (cw + gap) + cw / 2.0 - self.cam;
                    let cy = row_y + (1.0 - p) * 36.0 * s;
                    (cx, cy, p, self.focus[i])
                })
                .collect();

            // Glows go underneath every card.
            for (i, &(cx, cy, p, f)) in layout.iter().enumerate() {
                cv.glow(cx, cy, cw * 1.7, self.entries[i].icon.accent(), 0.6 * f * p);
            }
            for (i, &(cx, cy, p, f)) in layout.iter().enumerate() {
                if cx + cw < 0.0 || cx - cw > w {
                    continue;
                }
                self.draw_card(cv, text, i, cx, cy, cw, ch, s, p, f);
                self.hits.push((cx - cw / 2.0, cy - ch / 2.0, cw, ch, Target::Card(i)));
            }

            // ---- details for the selected entry -------------------------
            let e = &self.entries[self.sel];
            let a = clamp01(self.focus[self.sel] * 1.4 - 0.4) * layout[self.sel].2;
            let dy = row_y + ch / 2.0 + 64.0 * s;
            text.draw_centered(cv, Face::Body, 16.0 * s, &e.location, w / 2.0, dy, white, 0.62 * a);
            let file = match &e.options {
                Some(o) => format!("{}  {}", e.file, o),
                None => e.file.clone(),
            };
            let file = text.fit(Face::Body, 13.0 * s, &file, w * 0.6);
            text.draw_centered(cv, Face::Body, 13.0 * s, &file, w / 2.0, dy + 24.0 * s, white, 0.34 * a);

            if let Some((start, len)) = self.countdown {
                let left = (len - (now - start)).max(0.0);
                let frac = (left / len) as f32;
                let (bw, bh) = (260.0 * s, 4.0 * s);
                let (bx, by) = (w / 2.0 - bw / 2.0, dy + 58.0 * s);
                cv.rrect(bx, by, bw, bh, bh / 2.0, white, 0.12);
                cv.rrect(bx, by, bw * frac, bh, bh / 2.0, white, 0.85);
                let msg = format!("Starting {} in {}s · press any key to stay", e.title, libm::ceil(left) as u32);
                text.draw_centered(cv, Face::Body, 14.0 * s, &msg, w / 2.0, by + 30.0 * s, white, 0.55);
            }
        }

        self.draw_actions(cv, text, s);

        if let Some((x, y)) = self.cursor {
            draw_cursor(cv, x, y, 22.0 * s.max(0.8));
        }

        // ---- toast ----------------------------------------------------
        if let Some((msg, t, error)) = &self.toast {
            let age = (now - t) as f32;
            let a = clamp01(age / 0.2) * clamp01((4.5 - age) / 0.4);
            let size = 15.0 * s;
            let tw = text.width(Face::Body, size, msg) + 44.0 * s;
            let th = 44.0 * s;
            let (tx, ty) = (w / 2.0 - tw / 2.0, 36.0 * s + (1.0 - ease_out(clamp01(age / 0.3))) * -20.0 * s);
            cv.shadow(tx, ty + 6.0 * s, tw, th, th / 2.0, 24.0 * s, rgb(0, 0, 0), 0.4 * a);
            if *error {
                cv.rrect(tx, ty, tw, th, th / 2.0, rgb(196, 52, 64), 0.94 * a);
            } else {
                cv.rrect(tx, ty, tw, th, th / 2.0, rgb(40, 44, 60), 0.92 * a);
                cv.rrect_stroke(tx, ty, tw, th, th / 2.0, 1.0, white, 0.18 * a);
            }
            text.draw_centered(cv, Face::Body, size, msg, w / 2.0, ty + th / 2.0 + size * 0.36, white, a);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_card(&self, cv: &mut Canvas, text: &mut Text, i: usize, cx: f32, cy: f32, cw: f32, ch: f32, s: f32, p: f32, f: f32) {
        let e = &self.entries[i];
        let white = rgb(255, 255, 255);
        let scale = 1.0 + 0.07 * f;
        let (w, h) = (cw * scale, ch * scale);
        let (x, y) = (cx - w / 2.0, cy - h / 2.0);
        let r = 28.0 * s * scale;
        cv.shadow(x, y + 16.0 * s, w, h, r, 40.0 * s, rgb(0, 0, 0), 0.32 * p);
        cv.rrect(x, y, w, h, r, white, (0.045 + 0.08 * f) * p);
        // Subtle top-lit glass sheen.
        let (hw, hh) = (w / 2.0, h / 2.0);
        let sheen = (0.035 + 0.06 * f) * p;
        cv.paint_fading(
            (x, y, x + w, y + h * 0.55),
            |px, py| sd_rrect(px - cx, py - cy, hw, hh, r),
            |_, py| {
                let t = clamp01((py - y) / (h * 0.55));
                (white, sheen * (1.0 - t) * (1.0 - t))
            },
        );
        cv.rrect_stroke(x, y, w, h, r, 1.2 * s.max(1.0), white, (0.07 + 0.33 * f) * p);

        let icon = 116.0 * s * scale;
        icons::draw_tile(cv, text, e.icon, cx, y + h * 0.43, icon, p * (0.7 + 0.3 * f));

        let size = 19.0 * s;
        let label = text.fit(Face::Body, size, &e.title, w - 28.0 * s);
        text.draw_centered(cv, Face::Body, size, &label, cx, y + h - 30.0 * s * scale, white, p * (0.6 + 0.4 * f));

        // Quick-boot number key.
        if i < 9 {
            let digit = format!("{}", i + 1);
            text.draw(cv, Face::Body, 13.0 * s, &digit, x + 18.0 * s, y + 30.0 * s, white, p * (0.18 + 0.2 * f));
        }
    }

    fn draw_actions(&mut self, cv: &mut Canvas, text: &mut Text, s: f32) {
        let (w, h) = (cv.w as f32, cv.h as f32);
        let white = rgb(255, 255, 255);
        let size = 15.0 * s;
        let ph = 46.0 * s;
        let icon = 18.0 * s;
        let pad = 20.0 * s;
        let widths: Vec<f32> = self
            .actions
            .iter()
            .map(|a| pad + icon + 10.0 * s + text.width(Face::Body, size, a.label()) + pad)
            .collect();
        let gap = 14.0 * s;
        let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len() - 1) as f32;
        let mut x = w / 2.0 - total / 2.0;
        let y = h - 64.0 * s - ph;
        for (i, (&a, &pw)) in self.actions.iter().zip(&widths).enumerate() {
            let f = self.act_focus[i];
            cv.rrect(x, y, pw, ph, ph / 2.0, white, 0.05 + 0.11 * f);
            cv.rrect_stroke(x, y, pw, ph, ph / 2.0, 1.2 * s.max(1.0), white, 0.07 + 0.38 * f);
            let (ix, iy) = (x + pad + icon / 2.0, y + ph / 2.0);
            let ia = 0.6 + 0.4 * f;
            match a {
                Action::Firmware => icons::draw_gear(cv, ix, iy, icon * 1.1, white, ia),
                Action::Restart => icons::draw_restart(cv, ix, iy, icon, white, ia),
                Action::Shutdown => icons::draw_power(cv, ix, iy, icon, white, ia),
            }
            text.draw(cv, Face::Body, size, a.label(), x + pad + icon + 10.0 * s, y + ph / 2.0 + size * 0.36, white, ia);
            self.hits.push((x, y, pw, ph, Target::Action(i)));
            x += pw + gap;
        }
    }

    fn hint_text(&self, text: &Text) -> String {
        let (l, r) = if text.has('←') { ('←', '→') } else { ('<', '>') };
        format!("{l} {r}  Choose      Enter  Start      Tab  Power      F5  Rescan")
    }
}

fn step(v: &mut f32, target: f32, k: f32) -> bool {
    let d = target - *v;
    if libm::fabsf(d) < 0.001 {
        let changed = *v != target;
        *v = target;
        changed
    } else {
        *v += d * k;
        true
    }
}

fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

fn clock_strings(now: f64) -> (String, String, f64) {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    match uefi::runtime::get_time() {
        Ok(t) => {
            let (y, m, d) = (t.year() as i32, t.month() as usize, t.day() as i32);
            // Sakamoto's day-of-week.
            const OFF: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
            let yy = if m < 3 { y - 1 } else { y };
            let dow = (yy + yy / 4 - yy / 100 + yy / 400 + OFF[(m - 1) % 12] + d).rem_euclid(7) as usize;
            (
                format!("{:02}:{:02}", t.hour(), t.minute()),
                format!("{}, {} {}", DAYS[dow], MONTHS[(m - 1) % 12], d),
                now,
            )
        }
        Err(_) => (String::new(), String::new(), now),
    }
}
