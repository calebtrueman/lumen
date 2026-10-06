//! Keyboard (INT 16h, so USB keyboards work through the BIOS's legacy
//! support) and mouse (the BIOS's PS/2 pointing-device services, INT 15h
//! C2xx, which BIOSes also provide for USB mice).

use crate::bios::{self, R};
use crate::common::MOUSE_RING;
use lumen_core::input::Key;

pub fn key() -> Option<Key> {
    let r = bios::call(0x16, R { eax: 0x1100, ..Default::default() });
    if r.eflags & 0x40 != 0 {
        return None; // ZF: no key waiting
    }
    let r = bios::call(0x16, R { eax: 0x1000, ..Default::default() });
    let (scan, ascii) = ((r.eax >> 8) as u8, r.eax as u8);
    Some(match (ascii, scan) {
        (0x0D, _) => Key::Enter,
        (0x1B, _) => Key::Escape,
        (0x09, _) => Key::Tab,
        (0 | 0xE0, 0x48) => Key::Up,
        (0 | 0xE0, 0x50) => Key::Down,
        (0 | 0xE0, 0x4B) => Key::Left,
        (0 | 0xE0, 0x4D) => Key::Right,
        (0 | 0xE0, 0x47) => Key::Home,
        (0 | 0xE0, 0x4F) => Key::End,
        (0, 0x3F) => Key::F5,
        (c, _) if (0x20..0x7F).contains(&c) => Key::Char(c as char),
        _ => return None,
    })
}

pub struct Mouse {
    pub present: bool,
    pub x: f32,
    pub y: f32,
    read: u32,
    buttons: u8,
}

pub struct Event {
    pub moved: bool,
    pub clicked: bool,
}

impl Mouse {
    pub fn new(w: usize, h: usize) -> Self {
        let handler = bios::info().mouse_handler;
        let mut present = false;
        unsafe { core::ptr::write_volatile(MOUSE_RING as *mut u32, 0) };
        // Reset/initialise for 3-byte packets, install the handler, enable.
        let init = bios::call(0x15, R { eax: 0xC205, ebx: 0x0300, ..Default::default() });
        if !init.carry() {
            let set = bios::call(0x15, R { eax: 0xC207, ebx: handler & 0xFFFF, es: (handler >> 16) as u16, ..Default::default() });
            let on = bios::call(0x15, R { eax: 0xC200, ebx: 0x0100, ..Default::default() });
            present = !set.carry() && !on.carry();
        }
        Self { present, x: w as f32 / 2.0, y: h as f32 / 2.0, read: 0, buttons: 0 }
    }

    /// Stop the BIOS calling Lumen's handler (before leaving Lumen).
    pub fn disable(&mut self) {
        if self.present {
            bios::call(0x15, R { eax: 0xC200, ebx: 0x0000, ..Default::default() });
            self.present = false;
        }
    }

    pub fn poll(&mut self, w: usize, h: usize, scale: f32) -> Event {
        let mut ev = Event { moved: false, clicked: false };
        if !self.present {
            return ev;
        }
        let count = unsafe { core::ptr::read_volatile(MOUSE_RING as *const u32) };
        if count.wrapping_sub(self.read) > 64 {
            self.read = count.wrapping_sub(64);
        }
        while self.read != count {
            let slot = MOUSE_RING + 4 + (self.read as usize & 63) * 4;
            let p = unsafe { core::ptr::read_volatile(slot as *const [u8; 4]) };
            self.read = self.read.wrapping_add(1);
            let status = p[0];
            if status & 0x08 == 0 || status & 0xC0 != 0 {
                continue; // not a valid packet, or overflow
            }
            let dx = p[1] as i32 - if status & 0x10 != 0 { 256 } else { 0 };
            let dy = p[2] as i32 - if status & 0x20 != 0 { 256 } else { 0 };
            if dx != 0 || dy != 0 {
                let speed = 1.6 * scale.max(1.0);
                self.x = (self.x + dx as f32 * speed).clamp(0.0, w as f32 - 1.0);
                self.y = (self.y - dy as f32 * speed).clamp(0.0, h as f32 - 1.0);
                ev.moved = true;
            }
            let left = status & 1;
            if left != 0 && self.buttons & 1 == 0 {
                ev.clicked = true;
            }
            self.buttons = status & 7;
        }
        ev
    }
}
