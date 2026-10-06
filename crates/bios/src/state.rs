//! What Lumen remembers between starts (the UEFI build keeps these in
//! NVRAM): the last system started and the Auto-start delay. Stored in
//! the state sector of Lumen's own disk area.

use crate::common::{header, HEADER};
use crate::disk;
use alloc::string::String;

const MAGIC: &[u8; 4] = b"LSTA";

pub struct State {
    pub auto_start: i32,
    pub last: String,
}

fn lba() -> u64 {
    let h = unsafe { core::slice::from_raw_parts(HEADER as *const u8, 512) };
    u64::from_le_bytes(h[header::BASE_LBA..header::BASE_LBA + 8].try_into().unwrap()) + header::STATE
}

pub fn load() -> State {
    let mut s = [0u8; 512];
    let drive = crate::bios::info().drive;
    if !disk::read(drive, 512, lba(), &mut s) || &s[0..4] != MAGIC {
        return State { auto_start: -1, last: String::new() };
    }
    let auto_start = i32::from_le_bytes(s[4..8].try_into().unwrap());
    let len = (u16::from_le_bytes([s[8], s[9]]) as usize).min(400);
    State { auto_start, last: String::from_utf8_lossy(&s[10..10 + len]).into_owned() }
}

/// Written only when something changed, to spare the disk.
pub fn save(st: &State) {
    let current = load();
    if current.auto_start == st.auto_start && current.last == st.last {
        return;
    }
    let mut s = [0u8; 512];
    s[0..4].copy_from_slice(MAGIC);
    s[4..8].copy_from_slice(&st.auto_start.to_le_bytes());
    let last = &st.last.as_bytes()[..st.last.len().min(400)];
    s[8..10].copy_from_slice(&(last.len() as u16).to_le_bytes());
    s[10..10 + last.len()].copy_from_slice(last);
    disk::write_sector(crate::bios::info().drive, lba(), &s);
}
