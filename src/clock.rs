//! Monotonic high-resolution time from the CPU's cycle counter, calibrated
//! against the firmware's `Stall()` at startup.

use core::sync::atomic::{AtomicU64, Ordering};

static FREQ: AtomicU64 = AtomicU64::new(1);
static START: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "x86_64")]
fn raw() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[cfg(target_arch = "aarch64")]
fn raw() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) v) };
    v
}

pub fn init() {
    #[cfg(target_arch = "aarch64")]
    {
        let f: u64;
        unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) f) };
        FREQ.store(f.max(1), Ordering::Relaxed);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let t0 = raw();
        uefi::boot::stall(core::time::Duration::from_millis(25));
        FREQ.store(((raw() - t0) * 40).max(1), Ordering::Relaxed);
    }
    START.store(raw(), Ordering::Relaxed);
}

/// Seconds since `init`.
pub fn now() -> f64 {
    (raw().wrapping_sub(START.load(Ordering::Relaxed))) as f64 / FREQ.load(Ordering::Relaxed) as f64
}
