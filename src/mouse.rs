//! Mice, touchpads (relative pointers) and touchscreens / tablets (absolute
//! pointers), merged into one on-screen cursor.

use crate::discover::open;
use alloc::vec::Vec;
use uefi::boot::{self, ScopedProtocol};
use uefi::proto::console::pointer::{AbsolutePointer, Pointer};

#[derive(Default)]
pub struct Events {
    pub moved: bool,
    pub clicked: bool,
    /// Wheel notches, positive = down/right.
    pub scroll: i32,
}

pub struct Mouse {
    relative: Vec<ScopedProtocol<Pointer>>,
    absolute: Vec<ScopedProtocol<AbsolutePointer>>,
    pub x: f32,
    pub y: f32,
    was_down: bool,
    scroll_accum: f32,
}

impl Mouse {
    pub fn new(w: usize, h: usize) -> Self {
        let mut m = Self { relative: Vec::new(), absolute: Vec::new(), x: w as f32 / 2.0, y: h as f32 / 2.0, was_down: false, scroll_accum: 0.0 };
        m.reopen();
        m
    }

    /// (Re)discover pointer devices, e.g. after a USB mouse is plugged in.
    pub fn reopen(&mut self) {
        self.relative = boot::find_handles::<Pointer>().unwrap_or_default().into_iter().filter_map(open::<Pointer>).collect();
        self.absolute =
            boot::find_handles::<AbsolutePointer>().unwrap_or_default().into_iter().filter_map(open::<AbsolutePointer>).collect();
        for p in &mut self.relative {
            let _ = p.reset(false);
        }
        log::info!("pointer devices: {} relative, {} absolute", self.relative.len(), self.absolute.len());
        for p in &mut self.absolute {
            let _ = p.reset(false);
        }
    }

    pub fn poll(&mut self, w: usize, h: usize, scale: f32) -> Events {
        let mut ev = Events::default();
        let mut down = false;

        for p in &mut self.relative {
            let res = p.mode().resolution_x.max(1) as f32;
            let res_y = p.mode().resolution_y.max(1) as f32;
            let res_z = p.mode().resolution_z.max(1) as f32;
            let Ok(Some(s)) = p.read_state() else { continue };
            // Counts -> millimetres -> pixels, with gentle acceleration.
            let dx = s.relative_movement_x as f32 / res;
            let dy = s.relative_movement_y as f32 / res_y;
            let speed = libm::sqrtf(dx * dx + dy * dy);
            let gain = 2.6 * scale * (1.0 + (speed / 6.0).min(2.0));
            if dx != 0.0 || dy != 0.0 {
                self.x += dx * gain;
                self.y += dy * gain;
                ev.moved = true;
            }
            self.scroll_accum += s.relative_movement_z as f32 / res_z;
            down |= bool::from(s.left_button);
        }

        for p in &mut self.absolute {
            let m = *p.mode();
            let Ok(Some(s)) = p.read_state() else { continue };
            let span_x = (m.absolute_max_x.saturating_sub(m.absolute_min_x)).max(1) as f32;
            let span_y = (m.absolute_max_y.saturating_sub(m.absolute_min_y)).max(1) as f32;
            let nx = (s.current_x.saturating_sub(m.absolute_min_x)) as f32 / span_x * w as f32;
            let ny = (s.current_y.saturating_sub(m.absolute_min_y)) as f32 / span_y * h as f32;
            if libm::fabsf(nx - self.x) > 0.5 || libm::fabsf(ny - self.y) > 0.5 {
                self.x = nx;
                self.y = ny;
                ev.moved = true;
            }
            down |= s.active_buttons & 1 != 0;
        }

        self.x = self.x.clamp(0.0, w as f32 - 1.0);
        self.y = self.y.clamp(0.0, h as f32 - 1.0);
        ev.clicked = down && !self.was_down;
        self.was_down = down;
        if libm::fabsf(self.scroll_accum) >= 1.0 {
            ev.scroll = self.scroll_accum as i32;
            self.scroll_accum -= ev.scroll as f32;
        }
        ev
    }
}
