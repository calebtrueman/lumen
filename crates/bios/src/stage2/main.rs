//! Stage 2: loads the compressed menu payload from Lumen's disk area,
//! unpacks it to 32 MiB and starts it. Runs in 32-bit protected mode,
//! reading the disk through the real-mode BIOS (boot.s).

#![no_std]
#![no_main]

#[path = "../common.rs"]
mod common;

use common::*;
use core::arch::global_asm;
use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

global_asm!(include_str!("boot.s"));

unsafe extern "C" {
    fn bios_int(n: u32);
    fn chainload(drive: u32, si: u32) -> !;
    fn mouse_handler();
}

extern "C" fn bios_int_fn(n: u32) {
    unsafe { bios_int(n) }
}
extern "C" fn chainload_fn(drive: u32, si: u32) -> ! {
    unsafe { chainload(drive, si) }
}

/// Where the packed payload is collected before unpacking (16 MiB).
const PACKED: usize = 0x0100_0000;

#[unsafe(no_mangle)]
extern "C" fn stage2_main() -> ! {
    let info = unsafe { &mut *(BOOT_INFO as *mut BootInfo) };
    info.magic = BOOT_INFO_MAGIC;
    info.bios_int = bios_int_fn;
    info.chainload = chainload_fn;
    info.mouse_handler = (mouse_handler as *const () as usize as u32) & 0xFFFF; // segment 0
    let drive = info.drive;

    let header = unsafe { core::slice::from_raw_parts(HEADER as *const u8, 512) };
    let le32 = |o: usize| u32::from_le_bytes(header[o..o + 4].try_into().unwrap());
    let base = u64::from_le_bytes(header[header::BASE_LBA..header::BASE_LBA + 8].try_into().unwrap());
    let stage2_sectors = le32(header::STAGE2_SECTORS) as u64;
    let sectors = le32(header::PAYLOAD_SECTORS) as u64;
    let packed_len = le32(header::PAYLOAD_PACKED) as usize;
    let size = le32(header::PAYLOAD_SIZE) as usize;
    let crc = le32(header::PAYLOAD_CRC);
    if packed_len == 0 || packed_len > sectors as usize * 512 || size == 0 || size > 0x0400_0000 {
        fail(drive, base, "bad header");
    }

    // Read the packed payload, 64 KiB at a time.
    let mut lba = base + header::STAGE2 + stage2_sectors;
    let mut left = sectors;
    let mut at = PACKED;
    while left > 0 {
        let n = left.min((BOUNCE_SIZE / 512) as u64).min(127) as u16;
        if !read_sectors(bios_int_fn, drive, lba, n) {
            fail(drive, base, "disk read error");
        }
        unsafe { core::ptr::copy_nonoverlapping(BOUNCE as *const u8, at as *mut u8, n as usize * 512) };
        at += n as usize * 512;
        lba += n as u64;
        left -= n as u64;
    }

    let packed = unsafe { core::slice::from_raw_parts(PACKED as *const u8, packed_len) };
    let out = unsafe { core::slice::from_raw_parts_mut(PAYLOAD_BASE as *mut u8, size) };
    let mut state = DecompressorOxide::new();
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, _, written) = decompress(&mut state, packed, out, 0, flags);
    if status != TINFLStatus::Done || written != size || crc32(out) != crc {
        fail(drive, base, "payload damaged");
    }
    let entry: extern "C" fn() -> ! = unsafe { core::mem::transmute(PAYLOAD_BASE) };
    entry()
}

/// Something's wrong with Lumen's own data: say so briefly, then start the
/// PC's original boot loader so it still boots.
fn fail(drive: u8, base: u64, why: &str) -> ! {
    let mut r = Regs { eax: 0x0003, ..Default::default() };
    int(bios_int_fn, 0x10, &mut r);
    for b in "Lumen: ".bytes().chain(why.bytes()).chain(" - starting the previous boot loader.\r\n".bytes()) {
        let mut r = Regs { eax: 0x0E00 | b as u32, ebx: 7, ..Default::default() };
        int(bios_int_fn, 0x10, &mut r);
    }
    // ~2 s so the message can be read (INT 15h AH=86h, CX:DX microseconds).
    let mut r = Regs { eax: 0x8600, ecx: 0x1E, edx: 0x8480, ..Default::default() };
    int(bios_int_fn, 0x15, &mut r);
    if read_sectors(bios_int_fn, drive, base + header::ORIGINAL_MBR, 1) {
        unsafe { core::ptr::copy_nonoverlapping(BOUNCE as *const u8, 0x7C00 as *mut u8, 512) };
        chainload_fn(drive as u32, 0);
    }
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Stage 2 never allocates; the crate's dependencies just require an
/// allocator to exist.
struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOC: NoAlloc = NoAlloc;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    let info = unsafe { &*(BOOT_INFO as *const BootInfo) };
    let header = unsafe { core::slice::from_raw_parts(HEADER as *const u8, 512) };
    let base = u64::from_le_bytes(header[16..24].try_into().unwrap());
    fail(info.drive, base, "internal error")
}
