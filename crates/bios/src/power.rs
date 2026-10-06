//! Restart and power off without UEFI: ACPI (the firmware's tables say
//! which register and value turn the PC off), with APM and the keyboard
//! controller as older fallbacks.

use crate::bios::{self, R};

pub fn restart() -> ! {
    // Keyboard controller reset line, then the PCI reset register, then a
    // triple fault.
    for _ in 0..16 {
        if bios::inb(0x64) & 0x02 == 0 {
            break;
        }
        crate::clock::sleep(0.001);
    }
    bios::outb(0x64, 0xFE);
    crate::clock::sleep(0.2);
    bios::outb(0xCF9, 0x06);
    crate::clock::sleep(0.2);
    unsafe {
        let null = [0u8; 6];
        core::arch::asm!("lidt [{}]", "int3", in(reg) &null, options(noreturn));
    }
}

/// Returns only if the PC couldn't be switched off.
pub fn shutdown() {
    acpi_off();
    apm_off();
}

fn find_rsdp() -> Option<usize> {
    let ebda = (unsafe { core::ptr::read_volatile(0x40E as *const u16) } as usize) << 4;
    let ranges = [(ebda, ebda + 1024), (0xE0000, 0x100000)];
    for (start, end) in ranges {
        if start == 0 {
            continue;
        }
        let mut p = start & !15;
        while p + 20 <= end {
            let sig = unsafe { core::slice::from_raw_parts(p as *const u8, 8) };
            if sig == b"RSD PTR " {
                let sum = unsafe { core::slice::from_raw_parts(p as *const u8, 20) }.iter().fold(0u8, |a, b| a.wrapping_add(*b));
                if sum == 0 {
                    return Some(p);
                }
            }
            p += 16;
        }
    }
    None
}

unsafe fn rd32(a: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(a as *const u32) }
}

fn acpi_off() {
    let Some(rsdp) = find_rsdp() else { return };
    let rsdt = unsafe { rd32(rsdp + 16) } as usize;
    if rsdt == 0 || unsafe { core::slice::from_raw_parts(rsdt as *const u8, 4) } != b"RSDT" {
        return;
    }
    let len = unsafe { rd32(rsdt + 4) } as usize;
    let fadt = (36..len).step_by(4).map(|o| unsafe { rd32(rsdt + o) } as usize).find(|&t| {
        t != 0 && unsafe { core::slice::from_raw_parts(t as *const u8, 4) } == b"FACP"
    });
    let Some(fadt) = fadt else { return };
    let dsdt = unsafe { rd32(fadt + 40) } as usize;
    let smi_cmd = unsafe { rd32(fadt + 48) } as u16;
    let acpi_enable = unsafe { core::ptr::read_volatile((fadt + 52) as *const u8) };
    let pm1a = unsafe { rd32(fadt + 64) } as u16;
    let pm1b = unsafe { rd32(fadt + 68) } as u16;
    if pm1a == 0 || dsdt == 0 {
        return;
    }
    // \_S5 in the DSDT: NameOp "_S5_" PackageOp len count, then the
    // SLP_TYPa and SLP_TYPb values (each a byte, maybe with prefix 0x0A).
    let dlen = unsafe { rd32(dsdt + 4) } as usize;
    let d = unsafe { core::slice::from_raw_parts(dsdt as *const u8, dlen) };
    let Some(at) = d.windows(4).position(|w| w == b"_S5_") else { return };
    let mut p = at + 4;
    if d.get(p) != Some(&0x12) {
        return;
    }
    p += 1;
    p += ((d[p] >> 6) & 3) as usize + 1; // PkgLength
    p += 1; // NumElements
    let val = |p: &mut usize| -> u16 {
        if d.get(*p) == Some(&0x0A) {
            *p += 1;
        }
        let v = d.get(*p).copied().unwrap_or(0) as u16;
        *p += 1;
        v
    };
    let (typ_a, typ_b) = (val(&mut p), val(&mut p));
    // Switch to ACPI mode if the firmware hasn't (SCI_EN clear).
    if bios_inw(pm1a) & 1 == 0 && smi_cmd != 0 && acpi_enable != 0 {
        bios::outb(smi_cmd, acpi_enable);
        for _ in 0..300 {
            if bios_inw(pm1a) & 1 != 0 {
                break;
            }
            crate::clock::sleep(0.01);
        }
    }
    bios::outw(pm1a, (typ_a << 10) | (1 << 13));
    if pm1b != 0 {
        bios::outw(pm1b, (typ_b << 10) | (1 << 13));
    }
    crate::clock::sleep(1.0);
}

fn bios_inw(port: u16) -> u16 {
    let v: u16;
    unsafe { core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack)) };
    v
}

fn apm_off() {
    let r = bios::call(0x15, R { eax: 0x5300, ..Default::default() });
    if r.carry() {
        return;
    }
    bios::call(0x15, R { eax: 0x5301, ..Default::default() });
    bios::call(0x15, R { eax: 0x530E, ecx: 0x0102, ..Default::default() });
    bios::call(0x15, R { eax: 0x5307, ebx: 1, ecx: 3, ..Default::default() });
}
