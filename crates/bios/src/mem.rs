//! The BIOS memory map (INT 15h E820) and Lumen's heap.

use crate::bios::{self, R};
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

/// Usable RAM regions below 4 GiB: (start, end).
pub static mut USABLE: Vec<(u64, u64)> = Vec::new();

/// The heap: right after the payload, up to 192 MiB. Initrds are placed
/// above it, at the top of memory, where Linux wants them.
pub const HEAP_MAX: u64 = 192 << 20;

unsafe extern "C" {
    static __payload_end: u8;
}

pub struct Map {
    pub regions: [(u64, u64, u32); 64],
    pub count: usize,
}

pub fn e820() -> Map {
    let mut map = Map { regions: [(0, 0, 0); 64], count: 0 };
    let mut cont = 0u32;
    let (seg, off) = bios::seg_off(bios::BOUNCE);
    loop {
        let r = bios::call(
            0x15,
            R { eax: 0xE820, ebx: cont, ecx: 24, edx: 0x534D_4150, edi: off as u32, es: seg, ..Default::default() },
        );
        if r.carry() || r.eax != 0x534D_4150 {
            break;
        }
        let b = bios::bounce();
        let base = u64::from_le_bytes(b[0..8].try_into().unwrap());
        let len = u64::from_le_bytes(b[8..16].try_into().unwrap());
        let ty = u32::from_le_bytes(b[16..20].try_into().unwrap());
        if map.count < map.regions.len() && len > 0 {
            map.regions[map.count] = (base, base + len, ty);
            map.count += 1;
        }
        cont = r.ebx;
        if cont == 0 {
            break;
        }
    }
    map
}

/// Set up the heap inside the usable region that holds the payload.
/// Returns false if there isn't enough memory to run.
pub fn init(map: &Map) -> bool {
    let start = (unsafe { &__payload_end as *const u8 as u64 } + 0xFFF) & !0xFFF;
    let Some(&(_, end, _)) = map.regions[..map.count].iter().find(|r| r.2 == 1 && r.0 <= start && r.1 > start) else {
        return false;
    };
    let end = end.min(0xFFFF_F000);
    let size = (end - start).min(HEAP_MAX);
    if size < (32 << 20) {
        return false;
    }
    unsafe { HEAP.lock().init(start as *mut u8, size as usize) };
    let usable: Vec<(u64, u64)> =
        map.regions[..map.count].iter().filter(|r| r.2 == 1 && r.0 < 0x1_0000_0000).map(|r| (r.0, r.1.min(0x1_0000_0000))).collect();
    unsafe { USABLE = usable };
    true
}

/// End of the heap: memory above it (and below 4 GiB) is free for initrds.
pub fn heap_end() -> u64 {
    let h = HEAP.lock();
    h.bottom() as u64 + h.size() as u64
}

