//! Starting things: boot sectors (an MBR or a partition's boot sector,
//! exactly as the BIOS or a classic boot manager would run them) and Linux
//! kernels (the x86 boot protocol, 32-bit entry).

use crate::bios;
use crate::disk::{self, Drive, Partition};
use crate::mem;
use crate::video::Display;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use lumen_core::fs::{self, FileSystem};

/// Run a boot sector already loaded at 0x7C00 (leaves Lumen for good).
fn run_sector(drive: u8, si: u32) -> ! {
    Display::text_mode();
    (bios::info().chainload)(drive as u32, si)
}

/// Start a disk's MBR. For Lumen's own disk that's the original MBR it
/// replaced (kept in Lumen's area).
pub fn chain_mbr(drive: u8, original: Option<&[u8; 512]>) -> String {
    let mut sector = [0u8; 512];
    match original {
        Some(o) => sector.copy_from_slice(o),
        None => {
            if !disk::read(drive, 512, 0, &mut sector) {
                return String::from("couldn't read the disk");
            }
        }
    }
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return String::from("the disk isn't bootable");
    }
    unsafe { core::ptr::copy_nonoverlapping(sector.as_ptr(), 0x7C00 as *mut u8, 512) };
    run_sector(drive, 0)
}

/// Start a partition's boot sector, with DS:SI pointing at a partition
/// table entry describing it (some boot sectors use it to find themselves).
pub fn chain_partition(d: &Drive, p: &Partition) -> String {
    let mut sector = alloc::vec![0u8; d.sector_size as usize];
    if !disk::read(d.number, d.sector_size, p.start, &mut sector) {
        return String::from("couldn't read the partition");
    }
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return String::from("the partition isn't bootable");
    }
    let entry: [u8; 16] = {
        let mut e = [0u8; 16];
        e[0] = 0x80;
        e[4] = p.mbr_type.max(0x07);
        e[8..12].copy_from_slice(&(p.start.min(u32::MAX as u64) as u32).to_le_bytes());
        e[12..16].copy_from_slice(&(p.sectors.min(u32::MAX as u64) as u32).to_le_bytes());
        e
    };
    unsafe {
        core::ptr::copy_nonoverlapping(sector.as_ptr(), 0x7C00 as *mut u8, 512);
        core::ptr::copy_nonoverlapping(entry.as_ptr(), 0x7BE as *mut u8, 16);
    }
    run_sector(d.number, 0x7BE)
}

// --------------------------------------------------------------------- Linux

const BOOT_PARAMS: usize = 0x80000;
const CMDLINE: usize = 0x81000;
const CMDLINE_MAX: usize = 4095;
const KERNEL_AT: usize = 0x100000;

/// What the kernel needs to know about the screen Lumen leaves it.
pub struct Framebuffer {
    pub base: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub pitch: u16,
    pub rgb: [(u8, u8); 3],
}

/// Load a kernel and its initrds from `fsys` and start it. Returns only on
/// failure, with the reason.
pub fn linux(
    fsys: &mut dyn FileSystem,
    kernel_path: &str,
    initrd_paths: &[String],
    cmdline: &str,
    fb: Option<Framebuffer>,
    before_jump: &mut dyn FnMut(),
) -> String {
    let image = match fs::read_file(fsys, kernel_path, fs::IMAGE_LIMIT) {
        Ok(i) => i,
        Err(e) => return format!("couldn't read the kernel ({e:?})"),
    };
    if image.len() < 0x1000 || &image[0x202..0x206] != b"HdrS" {
        return String::from("not a Linux kernel for this PC");
    }
    let version = u16::from_le_bytes([image[0x206], image[0x207]]);
    if version < 0x0206 {
        return String::from("the kernel is too old (boot protocol < 2.06)");
    }
    let setup_sects = match image[0x1F1] {
        0 => 4,
        n => n as usize,
    };
    let pm_offset = (setup_sects + 1) * 512;
    let pm = image.get(pm_offset..).unwrap_or(&[]);
    let loadflags = image[0x211];
    if loadflags & 0x01 == 0 {
        return String::from("the kernel isn't a bzImage");
    }
    if KERNEL_AT + pm.len() >= crate::common::PAYLOAD_BASE {
        return String::from("the kernel is too large");
    }
    let code32_start = u32::from_le_bytes(image[0x214..0x218].try_into().unwrap());
    let initrd_max = match u32::from_le_bytes(image[0x22C..0x230].try_into().unwrap()) {
        0 => 0x37FF_FFFF,
        n => n,
    } as u64;
    let cmdline_size = (u32::from_le_bytes(image[0x238..0x23C].try_into().unwrap()) as usize).clamp(255, CMDLINE_MAX);

    // Initrds, concatenated (each padded to 4 bytes, like GRUB).
    let mut initrd: Vec<u8> = Vec::new();
    for p in initrd_paths {
        match fs::read_file(fsys, p, fs::IMAGE_LIMIT) {
            Ok(d) => {
                initrd.extend_from_slice(&d);
                initrd.resize(initrd.len().next_multiple_of(4), 0);
            }
            Err(e) => return format!("couldn't read {p} ({e:?})"),
        }
    }
    // The initrd goes as high as Linux allows, above Lumen's heap.
    let initrd_at = if initrd.is_empty() {
        0
    } else {
        let size = initrd.len() as u64;
        let floor = mem::heap_end();
        let usable = unsafe { &*(&raw const mem::USABLE) };
        let spot = usable
            .iter()
            .filter_map(|&(start, end)| {
                let top = end.min(initrd_max + 1);
                let at = top.checked_sub(size)? & !0xFFF;
                (at >= start.max(floor)).then_some(at)
            })
            .max();
        match spot {
            Some(at) => at,
            None => return String::from("not enough memory for the initrd"),
        }
    };

    // Zero page ("boot_params"): the setup header from the image, plus
    // what the boot loader fills in.
    let mut bp = alloc::vec![0u8; 4096];
    let header_end = (0x202 + image[0x201] as usize).min(0x290);
    bp[0x1F1..header_end].copy_from_slice(&image[0x1F1..header_end]);
    bp[0x210] = 0xFF; // type_of_loader: undefined/other
    bp[0x211] = loadflags | 0x80; // CAN_USE_HEAP
    bp[0x224..0x226].copy_from_slice(&0xFE00u16.to_le_bytes()); // heap_end_ptr
    bp[0x228..0x22C].copy_from_slice(&(CMDLINE as u32).to_le_bytes());
    bp[0x218..0x21C].copy_from_slice(&(initrd_at as u32).to_le_bytes());
    bp[0x21C..0x220].copy_from_slice(&(initrd.len() as u32).to_le_bytes());
    // Memory map.
    let map = mem::e820();
    let n = map.count.min(128);
    bp[0x1E8] = n as u8;
    for (i, &(start, end, ty)) in map.regions[..n].iter().enumerate() {
        let o = 0x2D0 + i * 20;
        bp[o..o + 8].copy_from_slice(&start.to_le_bytes());
        bp[o + 8..o + 16].copy_from_slice(&(end - start).to_le_bytes());
        bp[o + 16..o + 20].copy_from_slice(&ty.to_le_bytes());
    }
    // The screen: a VESA linear framebuffer the kernel can keep using.
    if let Some(f) = &fb {
        bp[0x0F] = 0x23; // VIDEO_TYPE_VLFB
        bp[0x12..0x14].copy_from_slice(&f.width.to_le_bytes());
        bp[0x14..0x16].copy_from_slice(&f.height.to_le_bytes());
        bp[0x16..0x18].copy_from_slice(&f.depth.to_le_bytes());
        bp[0x18..0x1C].copy_from_slice(&f.base.to_le_bytes());
        let size = (f.pitch as u32 * f.height as u32).div_ceil(65536);
        bp[0x1C..0x20].copy_from_slice(&size.to_le_bytes());
        bp[0x24..0x26].copy_from_slice(&f.pitch.to_le_bytes());
        for (i, (pos, len)) in f.rgb.iter().enumerate() {
            bp[0x26 + i * 2] = *len;
            bp[0x27 + i * 2] = *pos;
        }
        bp[0x32..0x34].copy_from_slice(&1u16.to_le_bytes()); // pages
    } else {
        bp[0x06] = 3; // orig_video_mode: 80x25 text
        bp[0x07] = 80;
        bp[0x0E] = 25;
        bp[0x0F] = 0x22; // VIDEO_TYPE_VGAC
    }

    let mut cmd = Vec::from(cmdline.as_bytes());
    cmd.truncate(cmdline_size);
    cmd.push(0);

    // Point of no return: nothing below may fail.
    before_jump();
    unsafe {
        core::ptr::copy_nonoverlapping(bp.as_ptr(), BOOT_PARAMS as *mut u8, 4096);
        core::ptr::copy_nonoverlapping(cmd.as_ptr(), CMDLINE as *mut u8, cmd.len());
        if !initrd.is_empty() {
            core::ptr::copy_nonoverlapping(initrd.as_ptr(), initrd_at as *mut u8, initrd.len());
        }
        core::ptr::copy(pm.as_ptr(), KERNEL_AT as *mut u8, pm.len());
        // Linux's 32-bit entry: ESI = boot_params, flat segments 0x10/0x18
        // (Lumen's own), interrupts off.
        core::arch::asm!(
            "cli",
            "mov esi, ecx",
            "xor ebp, ebp",
            "xor edi, edi",
            "xor ebx, ebx",
            "jmp eax",
            in("eax") code32_start,
            in("ecx") BOOT_PARAMS,
            options(noreturn)
        );
    }
}
