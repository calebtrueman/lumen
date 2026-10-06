//! What stage 2 and the payload share: fixed low-memory addresses, the
//! BootInfo block, the Lumen area header, and BIOS calls.

#![allow(dead_code)]

pub const BOOT_INFO: usize = 0x0500;
pub const HEADER: usize = 0x0800;
pub const REGS: usize = 0x0C00;
pub const MOUSE_RING: usize = 0x0E00;
pub const BOUNCE: usize = 0x20000;
pub const BOUNCE_SIZE: usize = 0x10000;
pub const PAYLOAD_BASE: usize = 0x0200_0000;

pub const BOOT_INFO_MAGIC: u32 = 0x4E4D_554C; // "LUMN"

/// Filled in by stage 2 for the payload.
#[repr(C)]
pub struct BootInfo {
    pub magic: u32,
    /// BIOS drive number Lumen was started from (written by the asm).
    pub drive: u8,
    pub _pad: [u8; 3],
    pub bios_int: extern "C" fn(u32),
    pub chainload: extern "C" fn(u32, u32) -> !,
    /// Real-mode far pointer (segment << 16 | offset) to the INT 15h C207
    /// mouse handler.
    pub mouse_handler: u32,
}

/// The first sector of Lumen's area on disk (written by the installer).
pub mod header {
    pub const MAGIC: &[u8; 8] = b"LUMENBIO";
    pub const BASE_LBA: usize = 16; // u64: this sector's own LBA
    pub const STAGE2_SECTORS: usize = 24; // u32, from base + 3
    pub const PAYLOAD_SECTORS: usize = 28; // u32, after stage 2
    pub const PAYLOAD_PACKED: usize = 32; // u32 bytes (deflate)
    pub const PAYLOAD_SIZE: usize = 36; // u32 bytes, unpacked
    pub const PAYLOAD_CRC: usize = 44; // u32, CRC-32 of the unpacked image
    pub const TOTAL_SECTORS: usize = 52; // u32, whole area
    /// Relative to base: the PC's original MBR, Lumen's saved state.
    pub const ORIGINAL_MBR: u64 = 1;
    pub const STATE: u64 = 2;
    pub const STAGE2: u64 = 3;
}

/// Registers for a BIOS call (layout matches boot.s: keep them in step).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Regs {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
    pub esi: u32,
    pub edi: u32,
    pub ebp: u32,
    pub ds: u16,
    pub es: u16,
    pub eflags: u32,
}

const _: () = assert!(core::mem::offset_of!(Regs, ds) == 28 && core::mem::offset_of!(Regs, es) == 30 && core::mem::offset_of!(Regs, eflags) == 32);

impl Regs {
    pub fn carry(&self) -> bool {
        self.eflags & 1 != 0
    }
}

/// Make one BIOS call.
pub fn int(bios_int: extern "C" fn(u32), n: u8, regs: &mut Regs) {
    unsafe {
        core::ptr::write_volatile(REGS as *mut Regs, *regs);
        bios_int(n as u32);
        *regs = core::ptr::read_volatile(REGS as *const Regs);
    }
}

/// A far pointer for real mode, for an address below 1 MiB.
pub fn seg_off(addr: usize) -> (u16, u16) {
    ((addr >> 4) as u16, (addr & 15) as u16)
}

/// Read sectors with INT 13h AH=42h into the bounce buffer
/// (at most 127 sectors, and they must fit in it).
pub fn read_sectors(bios_int: extern "C" fn(u32), drive: u8, lba: u64, count: u16) -> bool {
    debug_assert!(count as usize * 512 <= BOUNCE_SIZE);
    #[repr(C)]
    struct Dap {
        size: u8,
        zero: u8,
        count: u16,
        offset: u16,
        segment: u16,
        lba: u64,
    }
    const DAP: usize = 0x0D00;
    let (segment, offset) = seg_off(BOUNCE);
    unsafe { core::ptr::write_volatile(DAP as *mut Dap, Dap { size: 16, zero: 0, count, offset, segment, lba }) };
    let mut r = Regs { eax: 0x4200, edx: drive as u32, esi: DAP as u32, ..Default::default() };
    int(bios_int, 0x13, &mut r);
    !r.carry() && (r.eax >> 8) & 0xFF == 0
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
