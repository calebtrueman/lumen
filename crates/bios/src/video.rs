//! Graphics through VESA BIOS Extensions: pick a linear-framebuffer mode
//! (the monitor's native resolution from EDID when the card offers it, up
//! to 1920x1200), and present the canvas by writing changed rows straight
//! into video memory.

use crate::bios::{self, R};
use alloc::vec::Vec;
use lumen_core::gfx::{self, Canvas};

pub struct Mode {
    pub number: u16,
    pub width: usize,
    pub height: usize,
    pub bpp: u8,
    pub pitch: usize,
    pub lfb: usize,
    /// Field positions/sizes of red, green, blue.
    pub rgb: [(u8, u8); 3],
}

pub struct Display {
    pub mode: Mode,
    pub canvas: Canvas,
    pub backdrop: Vec<u32>,
    shown: Vec<u32>,
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn mode_info(number: u16) -> Option<Mode> {
    let (seg, off) = bios::seg_off(bios::BOUNCE);
    let r = bios::call(0x10, R { eax: 0x4F01, ecx: number as u32, edi: off as u32, es: seg, ..Default::default() });
    if r.eax & 0xFFFF != 0x004F {
        return None;
    }
    let b = bios::bounce();
    let attrs = rd16(b, 0);
    // Supported, graphics, linear framebuffer available.
    if attrs & 0x01 == 0 || attrs & 0x10 == 0 || attrs & 0x80 == 0 {
        return None;
    }
    let memory_model = b[0x1B];
    let bpp = b[0x19];
    if memory_model != 6 || !matches!(bpp, 15 | 16 | 24 | 32) {
        return None; // direct colour only
    }
    let pitch = if rd16(b, 0x32) != 0 { rd16(b, 0x32) } else { rd16(b, 0x10) } as usize;
    Some(Mode {
        number,
        width: rd16(b, 0x12) as usize,
        height: rd16(b, 0x14) as usize,
        bpp,
        pitch,
        lfb: rd32(b, 0x28) as usize,
        rgb: [(b[0x20], b[0x1F]), (b[0x22], b[0x21]), (b[0x24], b[0x23])],
    })
}

/// The monitor's preferred resolution, from its EDID (VBE/DDC).
fn edid_native() -> Option<(usize, usize)> {
    let (seg, off) = bios::seg_off(bios::BOUNCE);
    let r = bios::call(0x10, R { eax: 0x4F15, ebx: 1, ecx: 0, edx: 0, edi: off as u32, es: seg, ..Default::default() });
    if r.eax & 0xFFFF != 0x004F {
        return None;
    }
    let e = bios::bounce();
    if e[0..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
        return None;
    }
    let d = &e[0x36..0x48]; // first detailed timing = preferred mode
    if d[0] == 0 && d[1] == 0 {
        return None;
    }
    let w = d[2] as usize | ((d[4] as usize & 0xF0) << 4);
    let h = d[5] as usize | ((d[7] as usize & 0xF0) << 4);
    (w >= 640 && h >= 480).then_some((w, h))
}

impl Display {
    pub fn new() -> Option<Self> {
        let (seg, off) = bios::seg_off(bios::BOUNCE);
        let b = bios::bounce();
        b[0..4].copy_from_slice(b"VBE2");
        let r = bios::call(0x10, R { eax: 0x4F00, edi: off as u32, es: seg, ..Default::default() });
        if r.eax & 0xFFFF != 0x004F || &b[0..4] != b"VESA" || rd16(b, 4) < 0x0200 {
            return None;
        }
        // The mode list is a far pointer; copy it before reusing the buffer.
        let ptr = rd32(b, 0x0E);
        let list_addr = ((ptr >> 16) << 4) as usize + (ptr & 0xFFFF) as usize;
        let mut numbers = Vec::new();
        for i in 0..512 {
            let n = unsafe { core::ptr::read_volatile((list_addr + i * 2) as *const u16) };
            if n == 0xFFFF {
                break;
            }
            numbers.push(n);
        }
        let modes: Vec<Mode> = numbers.into_iter().filter_map(mode_info).filter(|m| m.width <= 1920 && m.height <= 1200).collect();
        let native = edid_native();
        let depth_rank = |m: &Mode| match m.bpp {
            32 => 3,
            24 => 2,
            _ => 1,
        };
        let pick = native
            .and_then(|(w, h)| modes.iter().filter(|m| m.width == w && m.height == h).max_by_key(|m| depth_rank(m)))
            .or_else(|| modes.iter().max_by_key(|m| (m.width * m.height, depth_rank(m))))?;
        let number = pick.number;
        let mode = mode_info(number)?;
        let r = bios::call(0x10, R { eax: 0x4F02, ebx: number as u32 | 0x4000, ..Default::default() });
        if r.eax & 0xFFFF != 0x004F {
            return None;
        }
        let mut canvas = Canvas::new(mode.width, mode.height);
        gfx::render_backdrop(&mut canvas);
        let backdrop = canvas.px.clone();
        Some(Self { mode, canvas, backdrop, shown: Vec::new() })
    }

    /// Forget what's on screen; the next present sends everything.
    pub fn invalidate(&mut self) {
        self.shown.clear();
    }

    /// Send changed rows (trimmed to their changed columns) to the screen.
    pub fn present(&mut self) -> usize {
        let (w, h) = (self.canvas.w, self.canvas.h);
        if self.shown.len() != w * h {
            for y in 0..h {
                self.write_row(y, 0, w);
            }
            self.shown = self.canvas.px.clone();
            return w * h;
        }
        let mut sent = 0;
        for y in 0..h {
            let row = y * w..(y + 1) * w;
            let (new, old) = (&self.canvas.px[row.clone()], &self.shown[row.clone()]);
            if new == old {
                continue;
            }
            let first = new.iter().zip(old).position(|(a, b)| a != b).unwrap_or(0);
            let last = w - new.iter().rev().zip(old.iter().rev()).position(|(a, b)| a != b).unwrap_or(0);
            self.write_row(y, first, last);
            self.shown[y * w + first..y * w + last].copy_from_slice(&self.canvas.px[y * w + first..y * w + last]);
            sent += last - first;
        }
        sent
    }

    fn write_row(&self, y: usize, x0: usize, x1: usize) {
        let m = &self.mode;
        let src = &self.canvas.px[y * m.width + x0..y * m.width + x1];
        let row = m.lfb + y * m.pitch;
        let standard = m.rgb == [(16, 8), (8, 8), (0, 8)];
        unsafe {
            match m.bpp {
                32 if standard => {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), (row + x0 * 4) as *mut u32, src.len());
                }
                32 => {
                    let dst = (row + x0 * 4) as *mut u32;
                    for (i, &p) in src.iter().enumerate() {
                        dst.add(i).write_volatile(self.pack(p));
                    }
                }
                24 => {
                    let dst = (row + x0 * 3) as *mut u8;
                    for (i, &p) in src.iter().enumerate() {
                        let v = self.pack(p);
                        dst.add(i * 3).write_volatile(v as u8);
                        dst.add(i * 3 + 1).write_volatile((v >> 8) as u8);
                        dst.add(i * 3 + 2).write_volatile((v >> 16) as u8);
                    }
                }
                _ => {
                    let dst = (row + x0 * 2) as *mut u16;
                    for (i, &p) in src.iter().enumerate() {
                        dst.add(i).write_volatile(self.pack(p) as u16);
                    }
                }
            }
        }
    }

    /// 0x00RRGGBB -> the mode's pixel layout.
    fn pack(&self, p: u32) -> u32 {
        let ch = [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF];
        let mut v = 0;
        for (c, (pos, size)) in ch.iter().zip(self.mode.rgb) {
            v |= (c >> (8 - size.min(8) as u32)) << pos;
        }
        v
    }

    /// Back to 80x25 text before handing over to a boot sector.
    pub fn text_mode() {
        bios::call(0x10, R { eax: 0x0003, ..Default::default() });
    }
}
