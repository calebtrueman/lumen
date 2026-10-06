//! The installer on synthetic disks (in memory).

use lumen_biosinstall::*;
use std::io;

struct Mem(Vec<u8>);

impl Disk for Mem {
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        let s = lba as usize * SECTOR;
        buf.copy_from_slice(self.0.get(s..s + buf.len()).ok_or(io::ErrorKind::UnexpectedEof)?);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> io::Result<()> {
        let s = lba as usize * SECTOR;
        self.0.get_mut(s..s + buf.len()).ok_or(io::ErrorKind::UnexpectedEof)?.copy_from_slice(buf);
        Ok(())
    }
    fn sectors(&self) -> u64 {
        (self.0.len() / SECTOR) as u64
    }
}

/// A fake Lumen image: template MBR + an area of `sectors`.
fn image(sectors: usize) -> Image {
    let mut b = vec![0u8; SECTOR * (1 + sectors)];
    b[..4].copy_from_slice(b"LMBR");
    b[SECTOR..SECTOR + 8].copy_from_slice(b"LUMENBIO");
    b[SECTOR + 52..SECTOR + 56].copy_from_slice(&(sectors as u32).to_le_bytes());
    for (i, x) in b[SECTOR * 4..].iter_mut().enumerate() {
        *x = (i % 251) as u8 + 1;
    }
    Image::parse(&b).unwrap()
}

fn mbr_disk(first_partition: u32, code: &[u8]) -> Mem {
    let mut d = vec![0u8; 8 << 20];
    d[..code.len()].copy_from_slice(code);
    d[0x1B8..0x1BC].copy_from_slice(&0xCAFEBABEu32.to_le_bytes());
    d[0x1BE] = 0x80;
    d[0x1BE + 4] = 0x07;
    d[0x1BE + 8..0x1BE + 12].copy_from_slice(&first_partition.to_le_bytes());
    d[0x1BE + 12..0x1BE + 16].copy_from_slice(&10000u32.to_le_bytes());
    d[510] = 0x55;
    d[511] = 0xAA;
    Mem(d)
}

const WINDOWS_CODE: &[u8] = b"\x33\xc0Invalid partition table\0Error loading operating system\0Missing operating system";

#[test]
fn windows_disk_install_uninstall() {
    let mut d = mbr_disk(2048, WINDOWS_CODE);
    let before = d.0[..SECTOR].to_vec();
    let img = image(700);
    let r = install(&mut d, &img).unwrap();
    assert_eq!(r.previous, "Windows boot code");
    assert!(r.base + 700 <= 2048 && r.base >= 1);
    assert_eq!(&d.0[..4], b"LMBR");
    assert_eq!(&d.0[0x1B8..512], &before[0x1B8..512], "signature and partition table kept");
    assert_eq!(status(&mut d).unwrap(), Status::Installed { base: r.base });
    assert!(uninstall(&mut d).unwrap());
    assert_eq!(&d.0[..SECTOR], &before[..], "original MBR back");
    assert!(d.0[SECTOR..2048 * SECTOR].iter().all(|&b| b == 0), "area cleared");
    assert_eq!(status(&mut d).unwrap(), Status::NotInstalled);
}

#[test]
fn partition_table_changes_survive_uninstall() {
    let mut d = mbr_disk(2048, WINDOWS_CODE);
    install(&mut d, &image(100)).unwrap();
    d.0[0x1CE + 4] = 0x83; // a second partition added later
    uninstall(&mut d).unwrap();
    assert_eq!(d.0[0x1CE + 4], 0x83);
    assert_eq!(&d.0[2..25], b"Invalid partition table");
}

#[test]
fn too_small_gap_refused() {
    let mut d = mbr_disk(63, WINDOWS_CODE);
    let before = d.0.clone();
    let e = install(&mut d, &image(700)).unwrap_err();
    assert!(e.to_string().contains("isn't enough free space"), "{e}");
    assert_eq!(d.0, before, "nothing written");
}

#[test]
fn data_in_gap_refused() {
    let mut d = mbr_disk(2048, WINDOWS_CODE);
    d.0[2000 * SECTOR] = 0x42; // some other program's data
    let before = d.0.clone();
    let e = install(&mut d, &image(700)).unwrap_err();
    assert!(e.to_string().contains("in use"), "{e}");
    assert_eq!(d.0, before);
}

#[test]
fn grub_core_left_alone_and_heal_after_grub_install() {
    // GRUB's boot.img in the MBR pointing at its core image at sector 1.
    let mut code = vec![0u8; 440];
    code[0x180..0x184].copy_from_slice(b"GRUB");
    code[0x5C..0x64].copy_from_slice(&1u64.to_le_bytes());
    let mut d = mbr_disk(2048, &code);
    // core.img: 120 sectors from sector 1 (diskboot.img's blocklist says 119 more).
    for s in 1..121 {
        d.0[s * SECTOR..(s + 1) * SECTOR].fill(0x99);
    }
    d.0[SECTOR + 0x1FC..SECTOR + 0x1FE].copy_from_slice(&119u16.to_le_bytes());
    let img = image(700);
    let r = install(&mut d, &img).unwrap();
    assert_eq!(r.previous, "GRUB");
    assert!(r.base >= 121 + 64);
    assert!(d.0[SECTOR..121 * SECTOR].iter().all(|&b| b == 0x99 || b == 119 || b == 0), "core untouched");

    // grub-install runs again: new boot.img in the MBR.
    let mut grub = d.0[..SECTOR].to_vec();
    grub[..440].copy_from_slice(&code);
    grub[0x100] = 0x77; // a slightly different GRUB build
    d.0[..SECTOR].copy_from_slice(&grub);
    assert_eq!(status(&mut d).unwrap(), Status::Displaced { base: r.base });
    assert_eq!(heal(&mut d, &img).unwrap(), Healed::Restored { previous: "GRUB" });
    assert_eq!(&d.0[..4], b"LMBR");
    // The new GRUB is now the "previous boot loader".
    let base = status_base(&mut d);
    assert_eq!(d.0[(base as usize + 1) * SECTOR + 0x100], 0x77);
    assert_eq!(heal(&mut d, &img).unwrap(), Healed::Fine);
}

#[test]
fn update_keeps_original_and_state() {
    let mut d = mbr_disk(2048, WINDOWS_CODE);
    let original = d.0[..SECTOR].to_vec();
    let r = install(&mut d, &image(300)).unwrap();
    // Lumen saved some state.
    d.0[(r.base as usize + 2) * SECTOR..(r.base as usize + 2) * SECTOR + 4].copy_from_slice(b"LSTA");
    // A bigger new version.
    let r2 = install(&mut d, &image(700)).unwrap();
    assert_eq!(r2.previous, "Windows boot code", "the original, not Lumen's own code");
    assert_eq!(&d.0[(r2.base as usize + 1) * SECTOR..(r2.base as usize + 2) * SECTOR], &original[..]);
    assert_eq!(&d.0[(r2.base as usize + 2) * SECTOR..(r2.base as usize + 2) * SECTOR + 4], b"LSTA");
    uninstall(&mut d).unwrap();
    assert_eq!(&d.0[..SECTOR], &original[..]);
}

#[test]
fn damaged_area_is_healed() {
    let mut d = mbr_disk(2048, WINDOWS_CODE);
    let img = image(300);
    let r = install(&mut d, &img).unwrap();
    d.0[(r.base as usize + 10) * SECTOR] ^= 0xFF;
    assert_eq!(status(&mut d).unwrap(), Status::Displaced { base: r.base });
    assert!(matches!(heal(&mut d, &img).unwrap(), Healed::Restored { .. }));
    assert_eq!(status(&mut d).unwrap(), Status::Installed { base: r.base });
    // Healing the area kept the real original, not Lumen's code.
    assert_eq!(&d.0[(r.base as usize + 1) * SECTOR + 2..(r.base as usize + 1) * SECTOR + 25], b"Invalid partition table");
}

#[test]
fn gpt_disk() {
    let mut d = mbr_disk(1, &[0xEB, 0x63]);
    d.0[0x1BE] = 0;
    d.0[0x1BE + 4] = 0xEE;
    // GPT header at LBA 1: entries at LBA 2, 128 x 128 bytes; one partition from 2048.
    let h = &mut d.0[SECTOR..2 * SECTOR];
    h[..8].copy_from_slice(b"EFI PART");
    h[40..48].copy_from_slice(&2048u64.to_le_bytes()); // first usable, as most tools set it
    h[72..80].copy_from_slice(&2u64.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    let e = &mut d.0[2 * SECTOR..2 * SECTOR + 128];
    e[0] = 1; // type GUID non-zero
    e[32..40].copy_from_slice(&2048u64.to_le_bytes());
    e[40..48].copy_from_slice(&9999u64.to_le_bytes());
    assert_eq!(gap(&mut d).unwrap(), (34, 2048));
    let r = install(&mut d, &image(700)).unwrap();
    assert!(r.base >= 34 && r.base + 700 <= 2048);
    assert_eq!(&d.0[SECTOR..SECTOR + 8], b"EFI PART", "GPT untouched");
}

fn status_base(d: &mut Mem) -> u64 {
    match status(d).unwrap() {
        Status::Installed { base } | Status::Displaced { base } => base,
        Status::NotInstalled => panic!("not installed"),
    }
}
