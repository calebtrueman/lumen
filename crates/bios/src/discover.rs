//! What can be started on a BIOS PC: Windows (its active NTFS partition),
//! Linux installs (started directly), USB drives and other bootable disks
//! (their own MBR), and the boot loader Lumen replaced.

use crate::common::{header, HEADER};
use crate::disk::{self, Drive, Partition};
use crate::{bios, os, ui};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use lumen_core::fs;
use lumen_core::linux;

pub enum Target {
    /// Run a disk's MBR; on Lumen's own disk, the original one it replaced.
    Mbr { drive: u8, original: bool },
    /// Run a partition's boot sector.
    Partition { drive: usize, part: Partition },
    /// Load the kernel directly; `fallback` if that fails.
    Linux { drive: usize, part: Partition, kernel: String, initrds: Vec<String>, cmdline: String, fallback: Option<Box<Target>> },
}

pub struct Entry {
    pub card: ui::Card,
    pub target: Target,
    pub utility: bool,
}

/// The original MBR Lumen saved when it was installed.
pub fn original_mbr() -> Option<[u8; 512]> {
    let h = unsafe { core::slice::from_raw_parts(HEADER as *const u8, 512) };
    let base = u64::from_le_bytes(h[header::BASE_LBA..header::BASE_LBA + 8].try_into().unwrap());
    let mut s = [0u8; 512];
    (disk::read(bios::info().drive, 512, base + header::ORIGINAL_MBR, &mut s) && s[510] == 0x55 && s[511] == 0xAA).then_some(s)
}

#[derive(PartialEq, Clone, Copy)]
enum MbrKind {
    Grub,
    Windows,
    Other,
    Empty,
}

fn mbr_kind(s: &[u8; 512]) -> MbrKind {
    let code = &s[..440];
    let has = |needle: &[u8]| code.windows(needle.len()).any(|w| w == needle);
    if code.iter().all(|&b| b == 0) {
        MbrKind::Empty
    } else if has(b"GRUB") {
        MbrKind::Grub
    } else if has(b"Invalid partition table") || has(b"Missing operating system") {
        MbrKind::Windows
    } else {
        MbrKind::Other
    }
}

fn disk_name(d: &Drive) -> String {
    let n = d.number - 0x7F;
    match d.interface.as_str() {
        "USB" => String::from("USB drive"),
        "" => format!("Disk {n}"),
        i if i.eq_ignore_ascii_case("nvme") => format!("NVMe disk {n}"),
        i => format!("{i} disk {n}"),
    }
}

fn location(d: &Drive, p: Option<&Partition>, label: &str) -> String {
    let mut s = disk_name(d);
    if let Some(p) = p {
        s = format!("{s} · Partition {}", p.number);
    }
    if !label.trim().is_empty() {
        s = format!("{} · {s}", label.trim());
    }
    s
}

/// The volume name of a CD/DVD image written to a USB stick (ISO 9660).
fn iso_label(d: &Drive) -> Option<String> {
    let mut pvd = alloc::vec![0u8; 2048];
    let lba = 32768 / d.sector_size as u64;
    if !disk::read(d.number, d.sector_size, lba, &mut pvd) || &pvd[1..6] != b"CD001" || pvd[0] != 1 {
        return None;
    }
    let label = String::from_utf8_lossy(&pvd[40..72]).trim().to_string();
    (!label.is_empty()).then_some(label)
}

fn is_windows_boot(d: &Drive, p: &Partition) -> bool {
    if !p.active && p.mbr_type != 0 {
        return false;
    }
    let mut s = alloc::vec![0u8; d.sector_size as usize];
    if !disk::read(d.number, d.sector_size, p.start, &mut s) {
        return false;
    }
    let has = |needle: &[u8]| s.windows(needle.len()).any(|w| w == needle);
    &s[3..11] == b"NTFS    " && (has(b"BOOTMGR") || has(b"NTLDR"))
}

pub fn scan(drives: &[Drive]) -> Vec<Entry> {
    let own = bios::info().drive;
    let original = original_mbr();
    let original_kind = original.as_ref().map(mbr_kind).unwrap_or(MbrKind::Empty);
    let mut entries = Vec::new();
    let mut parts: Vec<linux::Part> = Vec::new();
    let mut part_where: Vec<(usize, Partition, String)> = Vec::new();

    for (di, d) in drives.iter().enumerate() {
        let Some(table) = disk::table(d) else { continue };
        // A USB drive is one thing to start: its own boot code. Not every
        // BIOS says which disks are USB; a disk holding a CD/DVD image
        // (ISO 9660, as live and installer sticks do) is one either way.
        let iso = if d.number != own { iso_label(d) } else { None };
        if (d.usb() || iso.is_some()) && d.number != own {
            if table.mbr[510] == 0x55 && table.mbr[511] == 0xAA {
                let label = iso.unwrap_or_default();
                let (title, icon) = match os::identify(&label).filter(|o| o.name != "Linux").or_else(|| os::identify_text(label.as_bytes())) {
                    Some(o) => (format!("{} (USB)", o.name), o.icon),
                    None if !label.is_empty() => (label.clone(), os::DRIVE),
                    None => (String::from("USB Drive"), os::DRIVE),
                };
                entries.push(Entry {
                    card: card(format!("usb:{}:{}", d.number, label), title, location(d, None, ""), String::from("Starts from its own boot loader"), icon),
                    target: Target::Mbr { drive: d.number, original: false },
                    utility: true,
                });
            }
            continue;
        }
        let mut found_os = false;
        for p in &table.partitions {
            if is_windows_boot(d, p) {
                found_os = true;
                // On Lumen's disk, Windows' own MBR (if that's what was
                // there) starts it exactly as before Lumen.
                let target = if d.number == own && original_kind == MbrKind::Windows {
                    Target::Mbr { drive: own, original: true }
                } else {
                    Target::Partition { drive: di, part: p.clone() }
                };
                entries.push(Entry {
                    card: card(format!("win:{}", p.partuuid), String::from("Windows"), location(d, Some(p), ""), String::from("Windows Boot Manager"), os::WINDOWS),
                    target,
                    utility: false,
                });
                continue;
            }
            if let Some(f) = fs::open(disk::volume(d, p)) {
                part_where.push((di, p.clone(), f.label()));
                parts.push(linux::Part { fs: f, partuuid: p.partuuid.clone() });
            }
        }
        // Another disk with its own boot code but nothing Lumen recognises.
        if !found_os && d.number != own && table.mbr[510] == 0x55 && table.mbr[511] == 0xAA && mbr_kind(&table.mbr) != MbrKind::Empty {
            entries.push(Entry {
                card: card(format!("disk:{}", d.number), disk_name(d), location(d, None, ""), String::from("Starts from its own boot loader"), os::DRIVE),
                target: Target::Mbr { drive: d.number, original: false },
                utility: true,
            });
        }
    }

    let installs = linux::discover(&mut parts);
    let start = entries.iter().position(|e| e.utility).unwrap_or(entries.len());
    let mut linux_entries = Vec::new();
    for inst in installs {
        let (di, ref p, ref label) = part_where[inst.boot];
        let d = &drives[di];
        let fallback = (d.number == own && original_kind == MbrKind::Grub).then(|| Box::new(Target::Mbr { drive: own, original: true }));
        crate::dbg!("lumen: linux {:?} {} {} {:?} [{}]", inst.name, inst.version, inst.kernel, inst.initrds, inst.cmdline);
        linux_entries.push(Entry {
            card: card(
                inst.id(&parts),
                inst.name.clone(),
                location(d, Some(p), label),
                if inst.version.is_empty() { inst.kernel.clone() } else { format!("Linux {}", inst.version) },
                inst.os.map(|o| o.icon).unwrap_or(os::LINUX),
            ),
            target: Target::Linux {
                drive: di,
                part: p.clone(),
                kernel: inst.kernel,
                initrds: inst.initrds,
                cmdline: inst.cmdline,
                fallback,
            },
            utility: false,
        });
    }
    entries.splice(start..start, linux_entries);

    // The boot loader Lumen replaced: always reachable (Shift at power-on
    // does the same), unless a Windows card already starts it.
    let windows_uses_it = entries.iter().any(|e| matches!(e.target, Target::Mbr { original: true, .. }) && !e.utility);
    if matches!(original_kind, MbrKind::Grub | MbrKind::Other | MbrKind::Windows) && !windows_uses_it {
        let (title, detail) = match original_kind {
            MbrKind::Grub => ("GRUB", "The boot loader before Lumen"),
            MbrKind::Windows => ("Windows Boot", "The boot code before Lumen"),
            _ => ("Previous Loader", "The boot code before Lumen"),
        };
        entries.push(Entry {
            card: card(String::from("original-mbr"), title.to_string(), location(&drives[0], None, ""), detail.to_string(), os::GEAR),
            target: Target::Mbr { drive: own, original: true },
            utility: true,
        });
    }
    disambiguate(&mut entries);
    entries
}

/// Same-named entries (Windows on two disks, say) get their disk added.
fn disambiguate(entries: &mut [Entry]) {
    let titles: Vec<String> = entries.iter().map(|e| e.card.title.clone()).collect();
    for e in entries.iter_mut() {
        if titles.iter().filter(|t| **t == e.card.title).count() > 1 {
            let place = e.card.location.split(" · ").find(|p| p.contains("disk") || p.contains("Disk")).unwrap_or("").to_string();
            if !place.is_empty() {
                e.card.title = format!("{} ({place})", e.card.title);
            }
        }
    }
}

fn card(id: String, title: String, location: String, detail: String, icon: lumen_core::icons::Icon) -> ui::Card {
    ui::Card { id, title, location, detail, icon }
}
