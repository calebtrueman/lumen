//! Installing Lumen on a BIOS PC's disk (also used by the Windows
//! installer, the Linux installer's helper binary and the tests).
//!
//! Layout written (see crates/bios): the MBR's boot code (first 440
//! bytes; the disk signature and partition table stay as they are), and
//! Lumen's area at the end of the free gap before the first partition:
//! header, the original MBR, a state sector, stage 2, the packed payload.
//!
//! Safety rules:
//! - Lumen's area only goes where every sector is empty (or already holds
//!   Lumen), outside the GPT and every partition, after GRUB's core image.
//! - The area is written and read back before the MBR is touched; the MBR
//!   write is a single sector.
//! - Uninstalling puts the original boot code back (with the disk's
//!   current partition table) and clears the area.

use std::fmt;
use std::io;

pub const SECTOR: usize = 512;
const MAGIC: &[u8; 8] = b"LUMENBIO";
const BASE_AT: usize = 0x1B0; // in the MBR code: where Lumen's area starts
const CODE_LEN: usize = 440; // MBR boot code (before the disk signature)

/// A disk with 512-byte sectors.
pub trait Disk {
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> io::Result<()>;
    fn write(&mut self, lba: u64, buf: &[u8]) -> io::Result<()>;
    fn sectors(&self) -> u64;
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Not a disk Lumen can be installed on, with the reason.
    Unsuitable(String),
    BadImage,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "disk error: {e}"),
            Error::Unsuitable(why) => f.write_str(why),
            Error::BadImage => f.write_str("the Lumen BIOS image is damaged"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Lumen's BIOS image (target/bios/lumen-bios.img).
pub struct Image {
    template: [u8; SECTOR],
    area: Vec<u8>,
}

impl Image {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SECTOR * 4 || bytes.len() % SECTOR != 0 || &bytes[SECTOR..SECTOR + 8] != MAGIC {
            return Err(Error::BadImage);
        }
        let mut template = [0u8; SECTOR];
        template.copy_from_slice(&bytes[..SECTOR]);
        let area = bytes[SECTOR..].to_vec();
        let total = u32::from_le_bytes(area[52..56].try_into().unwrap()) as usize;
        if total * SECTOR != area.len() {
            return Err(Error::BadImage);
        }
        Ok(Self { template, area })
    }

    pub fn sectors(&self) -> u64 {
        (self.area.len() / SECTOR) as u64
    }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn read_sector(d: &mut dyn Disk, lba: u64) -> Result<[u8; SECTOR]> {
    let mut s = [0u8; SECTOR];
    d.read(lba, &mut s)?;
    Ok(s)
}

/// Where Lumen's area is, if this disk's MBR is Lumen's.
fn installed_base(d: &mut dyn Disk, mbr: &[u8; SECTOR]) -> Option<u64> {
    let base = le64(mbr, BASE_AT);
    if base == 0 || base >= d.sectors() {
        return None;
    }
    let h = read_sector(d, base).ok()?;
    (&h[0..8] == MAGIC && le64(&h, 16) == base).then_some(base)
}

#[derive(Debug, PartialEq)]
pub enum Status {
    NotInstalled,
    /// Lumen's code is in the MBR and its area is intact.
    Installed { base: u64 },
    /// Lumen's area is there but something else rewrote the MBR (e.g.
    /// grub-install, Windows' bootsect): `heal` puts Lumen back.
    Displaced { base: u64 },
}

pub fn status(d: &mut dyn Disk) -> Result<Status> {
    let mbr = read_sector(d, 0)?;
    if let Some(base) = installed_base(d, &mbr) {
        // Damaged area (e.g. a bigger GRUB core written over it) counts as
        // displaced: heal reinstalls it.
        return Ok(if area_ok(d, base)? { Status::Installed { base } } else { Status::Displaced { base } });
    }
    match find_area(d)? {
        Some(base) => Ok(Status::Displaced { base }),
        None => Ok(Status::NotInstalled),
    }
}

/// Looks for a Lumen area in the gap (when the MBR no longer points to it).
fn find_area(d: &mut dyn Disk) -> Result<Option<u64>> {
    let gap = gap(d)?;
    let mut buf = vec![0u8; SECTOR * 64];
    let mut lba = gap.0;
    while lba < gap.1 {
        let n = (gap.1 - lba).min(64);
        d.read(lba, &mut buf[..n as usize * SECTOR])?;
        for i in 0..n as usize {
            let s = &buf[i * SECTOR..(i + 1) * SECTOR];
            if &s[0..8] == MAGIC && le64(s, 16) == lba + i as u64 {
                return Ok(Some(lba + i as u64));
            }
        }
        lba += n;
    }
    Ok(None)
}

/// Is the area at `base` complete (stage 2 and payload intact)?
fn area_ok(d: &mut dyn Disk, base: u64) -> Result<bool> {
    let h = read_sector(d, base)?;
    let total = le32(&h, 52) as u64;
    let crc = le32(&h, 56);
    if total < 4 || base + total > d.sectors() {
        return Ok(false);
    }
    let mut body = vec![0u8; (total as usize - 3) * SECTOR];
    d.read(base + 3, &mut body)?;
    Ok(crc32(&body) == crc)
}

/// The free sectors before the first partition: [start, end).
pub fn gap(d: &mut dyn Disk) -> Result<(u64, u64)> {
    let mbr = read_sector(d, 0)?;
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Err(Error::Unsuitable("the disk has no partition table".into()));
    }
    let entries: Vec<(u8, u64, u64)> = (0..4)
        .map(|i| {
            let e = &mbr[0x1BE + i * 16..0x1CE + i * 16];
            (e[4], le32(e, 8) as u64, le32(e, 12) as u64)
        })
        .filter(|e| e.0 != 0 && e.2 != 0)
        .collect();
    if entries.iter().any(|e| e.0 == 0xEE) {
        let h = read_sector(d, 1)?;
        if &h[0..8] != b"EFI PART" {
            return Err(Error::Unsuitable("the GPT header is missing".into()));
        }
        let first_usable = le64(&h, 40);
        let (entries_lba, count, size) = (le64(&h, 72), le32(&h, 80) as u64, le32(&h, 84) as u64);
        let array_end = entries_lba + (count * size).div_ceil(SECTOR as u64);
        let mut buf = vec![0u8; ((count * size) as usize).div_ceil(SECTOR) * SECTOR];
        d.read(entries_lba, &mut buf)?;
        let mut first = u64::MAX;
        for i in 0..count as usize {
            let e = &buf[i * size as usize..(i + 1) * size as usize];
            if e[0..16].iter().any(|&b| b != 0) {
                first = first.min(le64(e, 32));
            }
        }
        if first == u64::MAX {
            return Err(Error::Unsuitable("the disk has no partitions".into()));
        }
        // Partitioning tools set the first usable sector to 2048 for
        // alignment; the sectors between the partition array and the first
        // partition belong to nothing (the emptiness check in `plan` guards
        // against anything else living there).
        let _ = first_usable;
        Ok((array_end.max(2), first))
    } else {
        let first = entries.iter().map(|e| e.1).min().ok_or_else(|| Error::Unsuitable("the disk has no partitions".into()))?;
        Ok((1, first))
    }
}

/// GRUB's core image in the gap (when the MBR holds GRUB's boot.img):
/// the sectors to leave alone, [start, end).
fn grub_core(d: &mut dyn Disk, mbr: &[u8; SECTOR]) -> Result<Option<(u64, u64)>> {
    if !mbr[..CODE_LEN].windows(4).any(|w| w == b"GRUB") {
        return Ok(None);
    }
    let start = le64(mbr, 0x5C);
    if start == 0 || start >= d.sectors() {
        return Ok(None);
    }
    let first = read_sector(d, start)?;
    // diskboot.img ends with a blocklist: u64 start, u16 sectors, u16 segment.
    let len = u16::from_le_bytes([first[0x1FC], first[0x1FD]]) as u64;
    Ok(Some((start, start + 1 + len)))
}

/// Where Lumen's area would go: as late in the gap as possible.
pub fn plan(d: &mut dyn Disk, image: &Image) -> Result<u64> {
    let mbr = read_sector(d, 0)?;
    let (mut start, end) = gap(d)?;
    if let Some((core_start, core_end)) = grub_core(d, &mbr)? {
        // GRUB's core in the gap (MBR disks; on GPT it has its own BIOS
        // boot partition): stay after it, with room for it to grow.
        if core_start < end {
            start = start.max(core_end + 64);
        }
    }
    let need = image.sectors();
    let base = end.checked_sub(need).map(|b| b & !7).unwrap_or(0);
    if base < start || base == 0 {
        let have = end.saturating_sub(start) * SECTOR as u64 / 1024;
        return Err(Error::Unsuitable(format!(
            "there isn't enough free space before the first partition ({have} KiB free, Lumen needs {} KiB). \
             Disks partitioned since Windows Vista have 1 MiB; very old ones don't.",
            need * SECTOR as u64 / 1024
        )));
    }
    // Everything there must be unused (or an older Lumen).
    let old = installed_base(d, &mbr).or(find_area(d)?);
    let mut buf = vec![0u8; need as usize * SECTOR];
    d.read(base, &mut buf)?;
    let overlaps_old = |lba: u64| old.is_some_and(|o| lba >= o && lba < o + 4096);
    for (i, s) in buf.chunks(SECTOR).enumerate() {
        if s.iter().any(|&b| b != 0) && !overlaps_old(base + i as u64) {
            return Err(Error::Unsuitable(format!(
                "the space before the first partition is in use (sector {} isn't empty); another program keeps data there",
                base + i as u64
            )));
        }
    }
    Ok(base)
}

#[derive(Debug)]
pub struct Installed {
    pub base: u64,
    /// What was in the MBR before (and stays reachable from Lumen).
    pub previous: &'static str,
}

/// Install (or update) Lumen on this disk.
pub fn install(d: &mut dyn Disk, image: &Image) -> Result<Installed> {
    let mbr = read_sector(d, 0)?;
    // The boot code to keep: what was there before Lumen. On an update,
    // that's the copy in the existing area, not Lumen's own code.
    let old = installed_base(d, &mbr).or(find_area(d)?);
    let original = match old {
        Some(o) if installed_base(d, &mbr).is_some() => read_sector(d, o + 1)?,
        _ => mbr,
    };
    let state = match old {
        Some(o) => Some(read_sector(d, o + 2)?),
        None => None,
    };
    let base = plan(d, image)?;
    write_area(d, image, base, &original, state)?;
    if let Some(o) = old {
        if o != base {
            clear_area(d, o, base)?;
        }
    }
    write_mbr(d, image, base)?;
    Ok(Installed { base, previous: describe(&original) })
}

fn write_area(d: &mut dyn Disk, image: &Image, base: u64, original: &[u8; SECTOR], state: Option<[u8; SECTOR]>) -> Result<()> {
    let mut area = image.area.clone();
    area[16..24].copy_from_slice(&base.to_le_bytes());
    area[SECTOR..2 * SECTOR].copy_from_slice(original);
    // Keep Lumen's saved state (last choice, Auto-start) across updates.
    if let Some(st) = state.filter(|s| &s[0..4] == b"LSTA") {
        area[2 * SECTOR..3 * SECTOR].copy_from_slice(&st);
    }
    let crc = crc32(&area[3 * SECTOR..]);
    area[56..60].copy_from_slice(&crc.to_le_bytes());
    d.write(base, &area)?;
    let mut check = vec![0u8; area.len()];
    d.read(base, &mut check)?;
    if check != area {
        return Err(Error::Io(io::Error::other("what was written to the disk didn't read back the same")));
    }
    Ok(())
}

fn write_mbr(d: &mut dyn Disk, image: &Image, base: u64) -> Result<()> {
    // Re-read: keep the partition table exactly as it is now.
    let mut mbr = read_sector(d, 0)?;
    mbr[..CODE_LEN].copy_from_slice(&image.template[..CODE_LEN]);
    mbr[BASE_AT..BASE_AT + 8].copy_from_slice(&base.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    d.write(0, &mbr)?;
    Ok(())
}

/// Zero an old area (except where it overlaps the new one).
fn clear_area(d: &mut dyn Disk, old: u64, new: u64) -> Result<()> {
    let h = read_sector(d, old)?;
    let total = (le32(&h, 52) as u64).clamp(1, 4096);
    let zero = [0u8; SECTOR];
    for lba in old..old + total {
        if lba < new || lba >= new + 4096 {
            d.write(lba, &zero)?;
        }
    }
    Ok(())
}

fn describe(mbr: &[u8; SECTOR]) -> &'static str {
    let code = &mbr[..CODE_LEN];
    let has = |n: &[u8]| code.windows(n.len()).any(|w| w == n);
    if code.iter().all(|&b| b == 0) {
        "nothing"
    } else if has(b"GRUB") {
        "GRUB"
    } else if has(b"Invalid partition table") || has(b"Missing operating system") {
        "Windows boot code"
    } else {
        "another boot loader"
    }
}

/// Remove Lumen: the boot code that was there before goes back.
pub fn uninstall(d: &mut dyn Disk) -> Result<bool> {
    let mut mbr = read_sector(d, 0)?;
    let ours = installed_base(d, &mbr);
    let Some(base) = ours.or(find_area(d)?) else { return Ok(false) };
    if ours.is_some() {
        let original = read_sector(d, base + 1)?;
        mbr[..CODE_LEN].copy_from_slice(&original[..CODE_LEN]);
        d.write(0, &mbr)?;
    }
    clear_area(d, base, u64::MAX)?;
    Ok(true)
}

#[derive(Debug, PartialEq)]
pub enum Healed {
    Fine,
    /// Something rewrote the MBR; Lumen took it back and kept the new boot
    /// code as the "previous boot loader".
    Restored { previous: &'static str },
    NotInstalled,
}

/// Run at shutdown by the heal tasks: keep Lumen in charge after other
/// installers (grub-install, Windows' bootsect) rewrite the MBR.
pub fn heal(d: &mut dyn Disk, image: &Image) -> Result<Healed> {
    match status(d)? {
        Status::NotInstalled => Ok(Healed::NotInstalled),
        Status::Installed { .. } => Ok(Healed::Fine),
        Status::Displaced { base } => {
            let mbr = read_sector(d, 0)?;
            let ours = installed_base(d, &mbr).is_some();
            // The MBR's current code is the boot loader to fall back to now
            // (unless it's still Lumen's and only the area was damaged).
            let original = if ours { read_sector(d, base + 1)? } else { mbr };
            let state = read_sector(d, base + 2)?;
            let new_base = plan(d, image)?;
            write_area(d, image, new_base, &original, Some(state))?;
            if new_base != base {
                clear_area(d, base, new_base)?;
            }
            write_mbr(d, image, new_base)?;
            Ok(Healed::Restored { previous: describe(&original) })
        }
    }
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

/// A disk (or disk image) opened as a file: /dev/sda, \\.\PhysicalDrive0,
/// or an image for tests.
pub struct FileDisk {
    file: std::fs::File,
    sectors: u64,
}

impl FileDisk {
    pub fn open(path: &str, sectors: Option<u64>) -> io::Result<Self> {
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let sectors = match sectors {
            Some(s) => s,
            None => file.seek(SeekFrom::End(0))? / SECTOR as u64,
        };
        Ok(Self { file, sectors })
    }
}

impl Disk for FileDisk {
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        self.file.seek(SeekFrom::Start(lba * SECTOR as u64))?;
        self.file.read_exact(buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        self.file.seek(SeekFrom::Start(lba * SECTOR as u64))?;
        self.file.write_all(buf)?;
        self.file.flush()
    }
    fn sectors(&self) -> u64 {
        self.sectors
    }
}
