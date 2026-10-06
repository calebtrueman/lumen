//! Disks through the BIOS (INT 13h extensions), and their partitions
//! (MBR with extended/logical partitions, or GPT).

use crate::bios::{self, R};
use crate::common;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use lumen_core::fs::{self, Volume};

pub struct Drive {
    pub number: u8,
    pub sectors: u64,
    pub sector_size: u32,
    /// Interface the BIOS reports (EDD 3.0), e.g. "USB", "SATA"; may be empty.
    pub interface: String,
}

impl Drive {
    pub fn usb(&self) -> bool {
        self.interface.eq_ignore_ascii_case("USB")
    }
}

/// Hard disks the BIOS offers (0x80 upwards), including USB drives that
/// its legacy USB support presents as disks.
pub fn drives() -> Vec<Drive> {
    let count = unsafe { core::ptr::read_volatile(0x475 as *const u8) }.clamp(1, 16);
    let mut out = Vec::new();
    // A couple beyond the count: some BIOSes don't count USB drives.
    for number in 0x80..0x80 + count + 2 {
        let r = bios::call(0x13, R { eax: 0x4100, ebx: 0x55AA, edx: number as u32, ..Default::default() });
        if r.carry() || r.ebx & 0xFFFF != 0xAA55 {
            continue;
        }
        let b = bios::bounce();
        b[..0x4A].fill(0);
        b[0..2].copy_from_slice(&0x4Au16.to_le_bytes());
        let (seg, off) = bios::seg_off(bios::BOUNCE);
        let r = bios::call(0x13, R { eax: 0x4800, edx: number as u32, esi: off as u32, ds: seg, ..Default::default() });
        if r.carry() {
            continue;
        }
        let b = bios::bounce();
        let sectors = u64::from_le_bytes(b[0x10..0x18].try_into().unwrap());
        let sector_size = u16::from_le_bytes([b[0x18], b[0x19]]) as u32;
        let len = u16::from_le_bytes([b[0], b[1]]);
        // EDD 3.0: "device path" signature 0xBEDD, then the interface name.
        let interface = if len >= 0x42 && b[0x1E] == 0xDD && b[0x1F] == 0xBE {
            String::from_utf8_lossy(&b[0x28..0x30]).trim_end_matches(['\0', ' ']).into()
        } else {
            String::new()
        };
        if sectors == 0 || !matches!(sector_size, 512 | 4096) {
            continue;
        }
        out.push(Drive { number, sectors, sector_size, interface });
    }
    out
}

/// Read whole sectors (any count) into `buf`.
pub fn read(drive: u8, sector_size: u32, mut lba: u64, buf: &mut [u8]) -> bool {
    let ss = sector_size as usize;
    let per_call = (bios::BOUNCE_SIZE / ss).min(127);
    let mut done = 0;
    while done < buf.len() {
        let n = ((buf.len() - done).div_ceil(ss)).min(per_call);
        if !common::read_sectors(bios::info().bios_int, drive, lba, n as u16) {
            // One retry: floppy-era BIOS advice, still true for USB.
            bios::call(0x13, R { eax: 0, edx: drive as u32, ..Default::default() });
            if !common::read_sectors(bios::info().bios_int, drive, lba, n as u16) {
                return false;
            }
        }
        let take = (n * ss).min(buf.len() - done);
        buf[done..done + take].copy_from_slice(&bios::bounce()[..take]);
        done += take;
        lba += n as u64;
    }
    true
}

/// Write one sector (Lumen's own state sector only).
pub fn write_sector(drive: u8, lba: u64, data: &[u8; 512]) -> bool {
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
    bios::bounce()[..512].copy_from_slice(data);
    let (segment, offset) = bios::seg_off(bios::BOUNCE);
    unsafe { core::ptr::write_volatile(DAP as *mut Dap, Dap { size: 16, zero: 0, count: 1, offset, segment, lba }) };
    let r = bios::call(0x13, R { eax: 0x4300, edx: drive as u32, esi: DAP as u32, ..Default::default() });
    !r.carry()
}

/// A partition (or whole disk) as a byte-addressable volume.
pub struct DriveVolume {
    pub drive: u8,
    pub sector_size: u32,
    pub start: u64,
    pub sectors: u64,
}

impl Volume for DriveVolume {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> fs::Result<()> {
        let ss = self.sector_size as u64;
        let end = offset.checked_add(buf.len() as u64).ok_or(fs::Error::Io)?;
        if end > self.sectors * ss {
            return Err(fs::Error::Io);
        }
        let first = offset / ss;
        let skip = (offset % ss) as usize;
        if skip == 0 && buf.len() as u64 % ss == 0 {
            return if read(self.drive, self.sector_size, self.start + first, buf) { Ok(()) } else { Err(fs::Error::Io) };
        }
        let count = (skip + buf.len()).div_ceil(ss as usize);
        let mut tmp = alloc::vec![0u8; count * ss as usize];
        if !read(self.drive, self.sector_size, self.start + first, &mut tmp) {
            return Err(fs::Error::Io);
        }
        buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
        Ok(())
    }
    fn size(&self) -> u64 {
        self.sectors * self.sector_size as u64
    }
}

#[derive(Clone)]
pub struct Partition {
    /// 1-based, as Linux numbers them (logical partitions from 5).
    pub number: u32,
    pub start: u64,
    pub sectors: u64,
    /// MBR type byte (0 for GPT).
    pub mbr_type: u8,
    pub active: bool,
    pub partuuid: String,
}

pub struct Table {
    /// The MBR as read (sector 0).
    pub mbr: [u8; 512],
    pub partitions: Vec<Partition>,
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn table(d: &Drive) -> Option<Table> {
    let ss = d.sector_size as usize;
    let mut s0 = alloc::vec![0u8; ss];
    if !read(d.number, d.sector_size, 0, &mut s0) {
        return None;
    }
    let mut mbr = [0u8; 512];
    mbr.copy_from_slice(&s0[..512]);
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Some(Table { mbr, partitions: Vec::new() });
    }
    let protective = (0..4).any(|i| mbr[0x1BE + i * 16 + 4] == 0xEE);
    if protective {
        if let Some(parts) = gpt(d) {
            return Some(Table { mbr, partitions: parts });
        }
    }
    let sig = le32(&mbr, 0x1B8);
    let mut parts = Vec::new();
    for i in 0..4 {
        let e = &mbr[0x1BE + i * 16..0x1CE + i * 16];
        let (ty, start, len) = (e[4], le32(e, 8) as u64, le32(e, 12) as u64);
        if ty == 0 || len == 0 {
            continue;
        }
        if matches!(ty, 0x05 | 0x0F | 0x85) {
            logical(d, start, sig, &mut parts);
            continue;
        }
        parts.push(Partition {
            number: i as u32 + 1,
            start,
            sectors: len,
            mbr_type: ty,
            active: e[0] == 0x80,
            partuuid: format!("{sig:08x}-{:02x}", i + 1),
        });
    }
    Some(Table { mbr, partitions: parts })
}

/// Logical partitions: a chain of extended boot records.
fn logical(d: &Drive, ext_start: u64, sig: u32, out: &mut Vec<Partition>) {
    let mut ebr_at = ext_start;
    let mut number = 5;
    let mut buf = alloc::vec![0u8; d.sector_size as usize];
    for _ in 0..64 {
        if !read(d.number, d.sector_size, ebr_at, &mut buf) || buf[510] != 0x55 || buf[511] != 0xAA {
            return;
        }
        let e = &buf[0x1BE..0x1CE];
        if e[4] != 0 && le32(e, 12) != 0 {
            out.push(Partition {
                number,
                start: ebr_at + le32(e, 8) as u64,
                sectors: le32(e, 12) as u64,
                mbr_type: e[4],
                active: false,
                partuuid: format!("{sig:08x}-{number:02x}"),
            });
            number += 1;
        }
        let next = &buf[0x1CE..0x1DE];
        if next[4] == 0 || le32(next, 8) == 0 {
            return;
        }
        ebr_at = ext_start + le32(next, 8) as u64;
    }
}

fn gpt(d: &Drive) -> Option<Vec<Partition>> {
    let ss = d.sector_size as usize;
    let mut h = alloc::vec![0u8; ss];
    if !read(d.number, d.sector_size, 1, &mut h) || &h[0..8] != b"EFI PART" {
        return None;
    }
    let entries_lba = le64(&h, 72);
    let count = le32(&h, 80).min(256) as usize;
    let size = le32(&h, 84) as usize;
    if !(128..=1024).contains(&size) {
        return None;
    }
    let mut buf = alloc::vec![0u8; (count * size).div_ceil(ss) * ss];
    if !read(d.number, d.sector_size, entries_lba, &mut buf) {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..count {
        let e = &buf[i * size..(i + 1) * size];
        if e[0..16].iter().all(|&b| b == 0) {
            continue;
        }
        let (first, last) = (le64(e, 32), le64(e, 40));
        if last < first {
            continue;
        }
        out.push(Partition {
            number: i as u32 + 1,
            start: first,
            sectors: last - first + 1,
            mbr_type: 0,
            active: false,
            partuuid: guid_string(&e[16..32]),
        });
    }
    Some(out)
}

/// A GPT GUID (mixed-endian on disk) as text.
pub fn guid_string(g: &[u8]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        le32(g, 0),
        u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]),
        g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

pub fn volume(d: &Drive, p: &Partition) -> Box<dyn Volume> {
    Box::new(DriveVolume { drive: d.number, sector_size: d.sector_size, start: p.start, sectors: p.sectors })
}
