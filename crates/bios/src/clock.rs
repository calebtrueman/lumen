//! Time: the CPU's time stamp counter, calibrated against the PC's
//! programmable interval timer (interrupts are off in protected mode, so
//! the BIOS tick count doesn't advance), and the real-time clock.

use crate::bios::{self, R};
use lumen_core::ui::WallTime;

static mut TICKS_PER_SEC: f64 = 1e9;
static mut START: u64 = 0;

pub fn init() {
    // PIT channel 2, gated through port 0x61 (speaker off), one-shot count
    // of 50 ms; then see how far the TSC moved.
    const COUNT: u16 = 59659; // 1193182 Hz * 0.05 s
    let mut best = u64::MAX;
    for _ in 0..3 {
        let p = bios::inb(0x61);
        bios::outb(0x61, (p & !0x02) | 0x01);
        bios::outb(0x43, 0xB0);
        bios::outb(0x42, (COUNT & 0xFF) as u8);
        bios::outb(0x42, (COUNT >> 8) as u8);
        let t0 = bios::rdtsc();
        let mut spins = 0u32;
        while bios::inb(0x61) & 0x20 == 0 {
            spins += 1;
            if spins > 50_000_000 {
                break;
            }
        }
        best = best.min(bios::rdtsc() - t0);
    }
    unsafe {
        if best > 1000 && best != u64::MAX {
            TICKS_PER_SEC = best as f64 / 0.05;
        }
        START = bios::rdtsc();
    }
}

/// Seconds since start.
pub fn now() -> f64 {
    unsafe { (bios::rdtsc() - START) as f64 / TICKS_PER_SEC }
}

pub fn sleep(secs: f64) {
    let until = now() + secs;
    while now() < until {
        core::hint::spin_loop();
    }
}

fn bcd(v: u32) -> u8 {
    ((v >> 4) * 10 + (v & 15)) as u8
}

/// The real-time clock (INT 1Ah), as the BIOS keeps it (usually local time).
pub fn wall_clock() -> Option<WallTime> {
    let t = bios::call(0x1A, R { eax: 0x0200, ..Default::default() });
    let d = bios::call(0x1A, R { eax: 0x0400, ..Default::default() });
    if t.carry() || d.carry() {
        return None;
    }
    let year = bcd((d.ecx >> 8) & 0xFF) as u16 * 100 + bcd(d.ecx & 0xFF) as u16;
    Some(WallTime {
        year,
        month: bcd((d.edx >> 8) & 0xFF),
        day: bcd(d.edx & 0xFF),
        hour: bcd((t.ecx >> 8) & 0xFF),
        minute: bcd(t.ecx & 0xFF),
    })
}
