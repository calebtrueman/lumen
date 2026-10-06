//! Calls into the real-mode BIOS (through stage 2's trampoline), port I/O
//! and the CPU's time stamp counter.

use crate::common::{self, BootInfo, Regs, BOOT_INFO};

pub use common::{seg_off, Regs as R, BOUNCE, BOUNCE_SIZE};

pub fn info() -> &'static BootInfo {
    unsafe { &*(BOOT_INFO as *const BootInfo) }
}

pub fn int(n: u8, regs: &mut Regs) {
    common::int(info().bios_int, n, regs)
}

pub fn call(n: u8, regs: Regs) -> Regs {
    let mut r = regs;
    int(n, &mut r);
    r
}

pub fn bounce() -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(BOUNCE as *mut u8, BOUNCE_SIZE) }
}

pub fn outb(port: u16, v: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) }
}

pub fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) };
    v
}

pub fn outw(port: u16, v: u16) {
    unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack)) }
}

pub fn rdtsc() -> u64 {
    unsafe { core::arch::x86::_rdtsc() }
}

/// Debug output to QEMU's debug console (port 0xE9), test builds only.
#[cfg(feature = "debugcon")]
pub fn debug(s: &str) {
    for b in s.bytes() {
        outb(0xE9, b);
    }
    outb(0xE9, b'\n');
}
#[cfg(not(feature = "debugcon"))]
pub fn debug(_: &str) {}

#[macro_export]
macro_rules! dbg {
    ($($t:tt)*) => { $crate::bios::debug(&alloc::format!($($t)*)) };
}
