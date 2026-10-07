//! Finds bootable operating systems by probing every mounted EFI file
//! system for well-known loader paths.

use crate::config::Config;
use crate::gfx::rgb;
use crate::icons::{Glyph, Icon};
use crate::linux_boot;
use crate::os;
use lumen_core::linux;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeSet;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol, SearchType};
use uefi::fs::{FileSystem, PathBuf};
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::media::PartitionSignature;
use uefi::proto::device_path::{DevicePath, DevicePathNodeEnum, DeviceType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{File, FileSystemVolumeLabel};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::ProtocolPointer;
use uefi::runtime::{self, VariableVendor};
use uefi::{cstr16, CString16, Handle};

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x64";
#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aa64";

pub struct Entry {
    pub title: String,
    /// Where it lives, e.g. "EFI system partition · Partition 1".
    pub location: String,
    pub file: String,
    pub icon: Icon,
    pub device_path: Box<DevicePath>,
    pub options: Option<String>,
    pub id: String,
    /// Utilities (shell, removable media) sort after operating systems.
    pub utility: bool,
    /// The firmware's own `Boot####` entry for this loader, if it has one.
    /// Used as a fallback: set `BootNext` and let the firmware boot it.
    pub boot_option: Option<u16>,
    /// Known only from NVRAM (file system not readable by firmware, e.g.
    /// APFS without a driver); can only be started via `BootNext`.
    pub nvram_only: bool,
    /// A Linux install Lumen starts itself (kernel + initrd).
    pub linux: Option<linux_boot::Target>,
    /// For a Linux card: whether `device_path` is the distro's own loader,
    /// used if starting the kernel directly fails.
    pub has_loader: bool,
    /// For a GRUB loader on the ESP: where it finds /boot.
    pub grub_root: Option<GrubRoot>,
}

/// Where Lumen itself was loaded from, so it doesn't list itself.
pub struct SelfImage {
    pub device: Option<Handle>,
    pub part_key: Option<String>,
    pub file: String,
}

impl SelfImage {
    pub fn get() -> Self {
        let img = open::<LoadedImage>(boot::image_handle());
        let device = img.as_ref().and_then(|i| i.device());
        let file = img.as_ref().and_then(|i| i.file_path().map(file_path_text)).unwrap_or_default();
        let part_key = device.and_then(|d| partition_key(&open::<DevicePath>(d)?));
        Self { device, part_key, file }
    }

    pub fn dir(&self) -> String {
        match self.file.rfind('\\') {
            Some(i) => self.file[..i].to_string(),
            None => "\\EFI\\lumen".into(),
        }
    }
}

/// Opens a protocol non-exclusively, so firmware drivers keep working.
pub fn open<P: ProtocolPointer + ?Sized>(handle: Handle) -> Option<ScopedProtocol<P>> {
    let params = OpenProtocolParams { handle, agent: boot::image_handle(), controller: None };
    unsafe { boot::open_protocol::<P>(params, OpenProtocolAttributes::GetProtocol) }.ok()
}

fn file_path_text(dp: &DevicePath) -> String {
    let mut s = String::new();
    for node in dp.node_iter() {
        if let Ok(DevicePathNodeEnum::MediaFilePath(f)) = node.as_enum() {
            let part = String::from_utf16_lossy(&f.path_name().to_vec());
            let part = part.trim_end_matches('\0');
            if !s.is_empty() && !s.ends_with('\\') && !part.starts_with('\\') {
                s.push('\\');
            }
            s.push_str(part);
        }
    }
    s
}

struct Volume {
    fs: FileSystem,
    part_key: Option<String>,
    path: Box<DevicePath>,
    label: String,
    location: String,
    removable: bool,
}

impl Volume {
    fn open(handle: Handle) -> Option<Self> {
        let path = open::<DevicePath>(handle)?.to_boxed();
        let mut sfs = open::<SimpleFileSystem>(handle)?;
        let label = sfs
            .open_volume()
            .ok()
            .and_then(|mut root| root.get_boxed_info::<FileSystemVolumeLabel>().ok())
            .map(|l| l.volume_label().to_string())
            .unwrap_or_default();
        let removable = open::<BlockIO>(handle).map(|b| b.media().is_removable_media()).unwrap_or(false);
        let (location, usb) = describe(&path, &label, removable);
        let part_key = partition_key(&path);
        Some(Self { fs: FileSystem::new(sfs), part_key, path, label, location, removable: removable || usb })
    }

    fn exists(&mut self, file: &str) -> bool {
        CString16::try_from(file).map(|p| self.fs.try_exists(PathBuf::from(p)).unwrap_or(false)).unwrap_or(false)
    }

    fn list(&mut self, dir: &str, want_dirs: bool) -> Vec<String> {
        let Ok(p) = CString16::try_from(dir) else { return Vec::new() };
        let Ok(iter) = self.fs.read_dir(PathBuf::from(p)) else { return Vec::new() };
        iter.filter_map(Result::ok)
            .filter(|info| info.is_directory() == want_dirs)
            .map(|info| info.file_name().to_string())
            .filter(|n| n != "." && n != "..")
            .collect()
    }

    fn entry(&self, title: String, file: String, icon: Icon, options: Option<String>, utility: bool) -> Entry {
        Entry {
            id: match &options {
                None => format!("{}:{}", self.part_key.as_deref().unwrap_or(&self.location), file.to_lowercase()),
                Some(o) => format!("{}:{}|{o}", self.part_key.as_deref().unwrap_or(&self.location), file.to_lowercase()),
            },
            device_path: file_device_path(&self.path, &file),
            location: self.location.clone(),
            title,
            file,
            icon,
            options,
            utility,
            boot_option: None,
            nvram_only: false,
            linux: None,
            has_loader: false,
            grub_root: None,
        }
    }
}

/// "Label · NVMe disk · Partition 2", and whether it's on USB.
fn describe(path: &DevicePath, label: &str, removable: bool) -> (String, bool) {
    let mut partition = None;
    let mut bus = None;
    for node in path.node_iter() {
        match node.as_enum() {
            Ok(DevicePathNodeEnum::MediaHardDrive(hd)) => partition = Some(hd.partition_number()),
            Ok(DevicePathNodeEnum::MessagingUsb(_) | DevicePathNodeEnum::MessagingUsbClass(_)) => bus = Some("USB drive"),
            Ok(DevicePathNodeEnum::MessagingNvmeNamespace(_)) => bus = bus.or(Some("NVMe disk")),
            Ok(DevicePathNodeEnum::MessagingSata(_) | DevicePathNodeEnum::MessagingAtapi(_)) => bus = bus.or(Some("SATA disk")),
            Ok(DevicePathNodeEnum::MessagingSd(_) | DevicePathNodeEnum::MessagingEmmc(_)) => bus = bus.or(Some("SD/eMMC")),
            Ok(DevicePathNodeEnum::MessagingScsi(_) | DevicePathNodeEnum::MessagingSasEx(_)) => bus = bus.or(Some("SCSI disk")),
            _ => {}
        }
    }
    let usb = bus == Some("USB drive");
    let bus = if removable && !usb { "Removable disk" } else { bus.unwrap_or("Internal disk") };
    let mut location = String::from(bus);
    if !label.trim().is_empty() {
        location = format!("{} · {}", label.trim(), location);
    }
    if let Some(p) = partition {
        location = format!("{location} · Partition {p}");
    }
    (location, usb)
}

/// Appends a file path node to a partition's device path.
pub fn file_device_path(dev: &DevicePath, file: &str) -> Box<DevicePath> {
    let mut buf = Vec::new();
    let mut b = DevicePathBuilder::with_vec(&mut buf);
    for node in dev.node_iter() {
        b = b.push(&node).expect("device path node");
    }
    let name = CString16::try_from(file).unwrap_or_default();
    b = b.push(&build::media::FilePath { path_name: &name }).expect("file path node");
    b.finalize().expect("device path").to_boxed()
}

fn titlecase(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Labels that say nothing about what's on a volume.
fn generic_label(label: &str) -> bool {
    let l = label.trim().to_lowercase();
    l.is_empty() || matches!(l.as_str(), "efi" | "esp" | "efi system" | "efisys" | "no name" | "boot" | "system" | "usb" | "untitled")
}

/// Titles of boot menu entries in a GRUB config or systemd-boot entry.
fn menu_titles(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.split(|&b| b == b'\n') {
        let t = line.trim_ascii_start();
        if t.starts_with(b"menuentry") || t.starts_with(b"title") || t.starts_with(b"submenu") {
            out.extend_from_slice(t);
            out.push(b'\n');
        }
    }
    out
}

impl Volume {
    fn read(&mut self, path: &str, max: u64) -> Option<Vec<u8>> {
        let p = PathBuf::from(CString16::try_from(path).ok()?);
        let size = self.fs.metadata(&p).ok()?.file_size();
        if size > max {
            return None;
        }
        self.fs.read(&p).ok()
    }

    /// Works out what OS a volume's loader belongs to when the folder name
    /// doesn't say (live/installer USBs, bare `\EFI\BOOT` loaders), from
    /// strongest to weakest evidence.
    fn identify_contents(&mut self, loader_dir: &str) -> Option<(&'static os::Os, bool)> {
        // Windows installation media.
        for f in ["\\sources\\install.wim", "\\sources\\install.esd", "\\sources\\install.swm", "\\sources\\boot.wim"] {
            if self.exists(f) {
                return Some((os::by_name("Windows")?, true));
            }
        }
        // Debian/Ubuntu-style disc info, e.g. "Linux Mint 22 Wilma - Release amd64".
        if let Some(found) = self.read("\\.disk\\info", 4096).and_then(|t| os::identify_text(&t)) {
            return Some((found, false));
        }
        // Boot menu titles from systemd-boot entries and GRUB configs.
        let mut titles = Vec::new();
        for name in self.list("\\loader\\entries", false) {
            if let Some(t) = self.read(&format!("\\loader\\entries\\{name}"), 64 * 1024) {
                titles.extend(menu_titles(&t));
            }
        }
        let mut configs = Vec::new();
        for f in [format!("{loader_dir}\\grub.cfg"), "\\EFI\\BOOT\\grub.cfg".into(), "\\boot\\grub\\grub.cfg".into(), "\\boot\\grub\\loopback.cfg".into()] {
            if let Some(t) = self.read(&f, 512 * 1024) {
                titles.extend(menu_titles(&t));
                configs.extend(t);
            }
        }
        if let Some(found) = os::identify_text(&titles).or_else(|| os::identify_text(&configs)) {
            return Some((found, false));
        }
        // A meaningful volume label ("Ubuntu 24.04 LTS amd64", "ARCH_202410").
        if !generic_label(&self.label) {
            if let Some(found) = os::identify(&self.label).filter(|o| o.name != "Linux") {
                return Some((found, false));
            }
        }
        // Finally the distro CA certificate inside its shim / GRUB.
        let up = ARCH.to_uppercase();
        for f in [format!("{loader_dir}\\BOOT{up}.EFI"), format!("{loader_dir}\\shim{ARCH}.efi"), format!("{loader_dir}\\grub{ARCH}.efi")] {
            if let Some(found) = self.read(&f, 8 << 20).and_then(|b| os::identify_signer(&b)) {
                return Some((found, false));
            }
        }
        None
    }
}

/// Loader file names inside `\EFI\<vendor>\`, in order of preference.
/// shim first: under Secure Boot it's the one the firmware will accept.
fn vendor_loaders() -> [String; 10] {
    [
        format!("shim{ARCH}.efi"),
        "shim.efi".into(), // openSUSE
        format!("grub{ARCH}.efi"),
        "grub.efi".into(),
        format!("systemd-boot{ARCH}.efi"),
        format!("refind_{ARCH}.efi"),
        "loader.efi".into(),            // FreeBSD
        format!("bootloader{ARCH}.efi"), // Clear Linux
        "xen.efi".into(),               // Qubes
        "elilo.efi".into(),             // Slackware
    ]
}

pub fn scan(me: &SelfImage, cfg: &Config) -> Vec<Entry> {
    let own_dir = me.dir().to_lowercase();
    let mut entries = Vec::new();
    let mut volumes = Vec::new();
    let handles = boot::find_handles::<SimpleFileSystem>().unwrap_or_default();

    for handle in handles {
        let Some(mut vol) = Volume::open(handle) else { continue };
        let is_self_volume = Some(handle) == me.device;
        let mut found = Vec::new();

        // The removable-media fallback loader.
        let fallback = format!("\\EFI\\BOOT\\BOOT{}.EFI", ARCH.to_uppercase());
        // Lumen itself may live in \EFI\BOOT (as the fallback loader or
        // behind shim there); never list ourselves.
        let is_self = is_self_volume && own_dir == "\\efi\\boot";

        // A USB stick or other removable drive is one thing to boot: the
        // loader it's designed to start from. Live and installer media often
        // also carry their base distro's folder (Bazzite's has \EFI\fedora),
        // which must not show up as a separate, wrongly named OS.
        if vol.removable && !is_self && vol.exists(&fallback) {
            let label = vol.label.trim().to_string();
            let (title, icon) = match vol.identify_contents("\\EFI\\BOOT") {
                Some((o, true)) => (format!("{} Setup (USB)", o.name), o.icon),
                Some((o, false)) => (format!("{} (USB)", o.name), o.icon),
                None if !generic_label(&label) => (label, os::DRIVE),
                None => ("USB Drive".into(), os::DRIVE),
            };
            found.push(vol.entry(title, fallback, icon, None, true));
            volumes.push(vol);
            entries.extend(found);
            continue;
        }

        if vol.exists("\\EFI\\Microsoft\\Boot\\bootmgfw.efi") {
            found.push(vol.entry("Windows".into(), "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(), os::WINDOWS, None, false));
        }
        if vol.exists("\\System\\Library\\CoreServices\\boot.efi") {
            let title = if generic_label(&vol.label) || vol.label.trim().eq_ignore_ascii_case("macintosh hd") {
                String::from("macOS")
            } else {
                format!("macOS ({})", vol.label.trim())
            };
            found.push(vol.entry(title, "\\System\\Library\\CoreServices\\boot.efi".into(), os::MACOS, None, false));
        }

        for dir in vol.list("\\EFI", true) {
            let lower = dir.to_lowercase();
            let path = format!("\\EFI\\{dir}");
            // "lumen": Lumen itself, or another copy of it on another disk.
            if matches!(lower.as_str(), "boot" | "microsoft" | "linux" | "tools" | "oem" | "dell" | "hp" | "lenovo" | "lumen")
                || (is_self_volume && path.to_lowercase() == own_dir)
            {
                continue;
            }
            let Some(file) = vendor_loaders().into_iter().map(|n| format!("{path}\\{n}")).find(|f| vol.exists(f)) else {
                continue;
            };
            let (title, icon) = match os::identify(&lower) {
                // systemd-boot and plain GRUB folders don't say which distro;
                // their menu entries usually do.
                Some(o) if o.name == "Linux" => match vol.identify_contents(&path) {
                    Some((o, _)) => (o.name.to_string(), o.icon),
                    None => (String::from("Linux"), os::LINUX),
                },
                Some(o) => (o.name.to_string(), o.icon),
                None => match vol.identify_contents(&path) {
                    Some((o, _)) => (o.name.to_string(), o.icon),
                    None => (titlecase(&dir), neutral_icon(&dir)),
                },
            };
            let mut e = vol.entry(title, file, icon, None, false);
            e.grub_root = vol.grub_root(&path, &e.file);
            found.push(e);
        }

        // Unified kernel images (systemd-boot "type 2" entries).
        for name in vol.list("\\EFI\\Linux", false) {
            if !name.to_lowercase().ends_with(".efi") {
                continue;
            }
            let stem = &name[..name.len() - 4];
            let (title, icon) = match os::identify(stem) {
                Some(o) if o.name != "Linux" => (o.name.to_string(), o.icon),
                _ => (format!("Linux ({stem})"), os::LINUX),
            };
            found.push(vol.entry(title, format!("\\EFI\\Linux\\{name}"), icon, None, false));
        }

        for shell in [format!("\\EFI\\tools\\shell{ARCH}.efi"), format!("\\shell{ARCH}.efi")] {
            if vol.exists(&shell) {
                found.push(vol.entry("UEFI Shell".into(), shell, os::SHELL, None, true));
                break;
            }
        }

        if is_self_volume {
            for c in &cfg.entries {
                if vol.exists(&c.path) {
                    let icon = os::identify(&c.title).map(|o| o.icon).unwrap_or(os::LINUX);
                    found.push(vol.entry(c.title.clone(), c.path.clone(), icon, c.options.clone(), false));
                }
            }
        }

        // An internal disk with nothing else recognisable on it may still have
        // the fallback loader.
        if !is_self && found.is_empty() && vol.exists(&fallback) {
            let (title, icon) = match vol.identify_contents("\\EFI\\BOOT") {
                Some((o, true)) => (format!("{} Setup", o.name), o.icon),
                Some((o, false)) => (o.name.to_string(), o.icon),
                None => ("Boot Loader".into(), os::DRIVE),
            };
            let mut e = vol.entry(title, fallback, icon, None, false);
            // Some distros (openSUSE cloud images) install only here, with
            // the same grub.cfg stub a vendor folder would have.
            e.grub_root = vol.grub_root("\\EFI\\BOOT", &e.file);
            found.push(e);
        }

        volumes.push(vol);
        entries.extend(found);
    }

    merge_firmware_entries(&mut entries, &mut volumes, me);
    if cfg.linux_direct {
        add_linux(&mut entries);
    }

    entries.retain(|e| {
        let hay = format!("{} {}", e.title, e.file).to_lowercase();
        !cfg.hide.iter().any(|h| hay.contains(h.as_str()))
    });
    // Stable sort: operating systems first, utilities last.
    entries.sort_by_key(|e| e.utility);
    disambiguate(&mut entries);
    unique_ids(&mut entries);
    for e in &entries {
        log::info!("entry {:?} [{}] {}", e.title, e.id, e.location);
    }
    entries
}

/// Firmware in "fast boot" mode often only connects the boot disk. Connect
/// every controller so second drives, NVMe and USB disks all expose their
/// partitions before we look for operating systems.
pub fn connect_all() {
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::AllHandles) {
        for &h in handles.iter() {
            let _ = boot::connect_controller(h, &[], None, true);
        }
    }
}

/// A stable identity for a partition: its GPT GUID (or MBR signature).
fn partition_key(dp: &DevicePath) -> Option<String> {
    dp.node_iter().find_map(|node| match node.as_enum() {
        Ok(DevicePathNodeEnum::MediaHardDrive(hd)) => Some(match hd.partition_signature() {
            PartitionSignature::Guid(g) => format!("{g}"),
            PartitionSignature::Mbr(s) => format!("{:02x}{:02x}{:02x}{:02x}-{}", s[0], s[1], s[2], s[3], hd.partition_number()),
            _ => format!("part{}@{}", hd.partition_number(), hd.partition_start()),
        }),
        _ => None,
    })
}

struct BootOption {
    num: u16,
    desc: String,
    path: Box<DevicePath>,
}

/// Reads the firmware boot menu (`BootOrder` + `Boot####`). Every OS
/// installer registers itself here, so this catches loaders at paths the
/// file system scan doesn't know about.
fn firmware_boot_options() -> Vec<BootOption> {
    let Ok((order, _)) = runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pair in order.chunks_exact(2) {
        let num = u16::from_le_bytes([pair[0], pair[1]]);
        let Ok(name) = CString16::try_from(format!("Boot{num:04X}").as_str()) else { continue };
        let Ok((data, _)) = runtime::get_variable_boxed(&name, &VariableVendor::GLOBAL_VARIABLE) else { continue };
        // EFI_LOAD_OPTION: u32 attributes, u16 path length, UCS-2 description, device path.
        if data.len() < 8 {
            continue;
        }
        let active = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) & 1 != 0;
        let path_len = u16::from_le_bytes([data[4], data[5]]) as usize;
        let mut desc_units = Vec::new();
        let mut i = 6;
        while i + 1 < data.len() {
            let u = u16::from_le_bytes([data[i], data[i + 1]]);
            i += 2;
            if u == 0 {
                break;
            }
            desc_units.push(u);
        }
        let Some(path_bytes) = data.get(i..i + path_len) else { continue };
        let Ok(path) = <&DevicePath>::try_from(path_bytes) else { continue };
        if active {
            out.push(BootOption { num, desc: String::from_utf16_lossy(&desc_units), path: path.to_boxed() });
        }
    }
    out
}

/// Partition keys of every partition the firmware can see, readable or not.
fn present_partitions() -> BTreeSet<String> {
    boot::find_handles::<BlockIO>()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|h| partition_key(&open::<DevicePath>(h)?))
        .collect()
}

fn merge_firmware_entries(entries: &mut Vec<Entry>, volumes: &mut [Volume], me: &SelfImage) {
    let present = present_partitions();
    for opt in firmware_boot_options() {
        log::info!("firmware entry Boot{:04X} {:?}: {}", opt.num, opt.desc, opt.path);
        // Only entries pointing at a file on a partition; skips network
        // boot, legacy BIOS (BBS) entries and firmware built-ins.
        let file = file_path_text(&opt.path);
        let Some(key) = partition_key(&opt.path) else {
            log::info!("  skipped: not on a disk partition");
            continue;
        };
        if file.is_empty() || opt.path.node_iter().any(|n| n.device_type() == DeviceType::BIOS_BOOT_SPEC) {
            continue;
        }
        let id = format!("{key}:{}", file.to_lowercase());

        // Already found by the scan: just remember its firmware entry.
        if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
            log::info!("  matches scanned entry {:?}", e.title);
            e.boot_option.get_or_insert(opt.num);
            continue;
        }
        // Partition signatures aren't always unique (cloned disks), so the
        // volume must also actually contain the loader.
        let vol = volumes
            .iter_mut()
            .filter(|v| v.part_key.as_deref() == Some(key.as_str()))
            .find_map(|v| v.exists(&file).then_some(&*v));
        let is_self = me.part_key.as_deref() == Some(key.as_str()) && me.file.eq_ignore_ascii_case(&file);
        if is_self || opt.desc.to_lowercase().contains("lumen") || file.to_lowercase().starts_with("\\efi\\lumen\\") {
            continue;
        }
        if vol.is_none() && !present.contains(&key) {
            log::info!("  skipped: partition {key} no longer exists");
            continue;
        }

        let desc = opt.desc.trim();
        let (title, icon) = match os::identify(desc).or_else(|| os::identify(&file)) {
            Some(o) if o.name != "Linux" => (o.name.to_string(), o.icon),
            Some(o) if !desc.is_empty() => (desc.to_string(), o.icon),
            _ if !desc.is_empty() => (desc.to_string(), neutral_icon(desc)),
            _ => (String::from("Boot Loader"), os::DRIVE),
        };
        let (device_path, location, nvram_only) = match vol {
            Some(v) => (file_device_path(&v.path, &file), v.location.clone(), false),
            None => (opt.path.to_boxed(), String::from("Firmware boot entry"), true),
        };
        entries.push(Entry {
            title,
            location,
            file,
            icon,
            device_path,
            options: None,
            id,
            utility: false,
            boot_option: Some(opt.num),
            nvram_only,
            linux: None,
            has_loader: false,
            grub_root: None,
        });
    }
}

/// Monogram for an OS we don't recognise.
fn neutral_icon(name: &str) -> Icon {
    let letter = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_ascii_uppercase();
    Icon { glyph: Glyph::Letter(letter), top: rgb(130, 140, 160), bottom: rgb(70, 78, 96), ink: rgb(255, 255, 255), glow: None }
}

/// Two entries with the same title (e.g. Windows on two disks, or an
/// installed distro and its live USB) get the loader or volume appended.
fn disambiguate(entries: &mut [Entry]) {
    let renamed: Vec<Option<String>> = entries
        .iter()
        .map(|e| {
            let same_title = entries.iter().filter(|o| o.title == e.title).count();
            if same_title < 2 {
                return None;
            }
            let same_volume = entries.iter().filter(|o| o.title == e.title && o.location == e.location).count();
            let extra = if same_volume > 1 {
                e.file.rsplit('\\').next().unwrap_or("")
            } else {
                e.location.split(" · ").next().unwrap_or("")
            };
            Some(format!("{} ({extra})", e.title))
        })
        .collect();
    for (e, t) in entries.iter_mut().zip(renamed) {
        if let Some(t) = t {
            e.title = t;
        }
    }
}

/// Two USB sticks flashed from the same image carry identical partition
/// signatures, so their ids collide. Suffix repeats so that "remember last
/// choice" and hot-plug tracking stay unambiguous.
fn unique_ids(entries: &mut [Entry]) {
    for i in 1..entries.len() {
        let repeats = entries[..i].iter().filter(|e| e.id == entries[i].id || e.id.starts_with(&format!("{}#", entries[i].id))).count();
        if repeats > 0 {
            entries[i].id = format!("{}#{}", entries[i].id, repeats + 1);
        }
    }
}

/// Whether a loader's title and an install's name are the same OS:
/// equal, or one is the other plus an edition ("openSUSE" for "openSUSE
/// Tumbleweed", identified from the loader's signing CA).
fn same_os(loader: &str, install: &str) -> bool {
    let (a, b) = (loader.to_lowercase(), install.to_lowercase());
    let prefix = |short: &str, long: &str| long.strip_prefix(short).is_some_and(|rest| rest.starts_with(' '));
    a == b || prefix(&a, &b) || prefix(&b, &a)
}

/// How a GRUB loader on the ESP finds the file system with /boot.
pub enum GrubRoot {
    /// From the `grub.cfg` stub next to it (most distros).
    Uuid(String),
    /// From the prefix built into GRUB's image, e.g. "(,gpt3)/boot/grub"
    /// (Arch's `grub-install`): that partition on the loader's own disk.
    Partition(u32),
}

impl Volume {
    fn grub_root(&mut self, dir: &str, loader: &str) -> Option<GrubRoot> {
        if let Some(u) = self.read(&format!("{dir}\\grub.cfg"), 64 * 1024).and_then(|t| grub_stub_root(&t)) {
            return Some(GrubRoot::Uuid(u));
        }
        self.read(loader, 8 << 20).and_then(|b| grub_embedded_partition(&b)).map(GrubRoot::Partition)
    }
}

/// The partition number in a GRUB image's built-in prefix: "(,gpt3)" or
/// "(hd0,msdos2)" -> 3 / 2.
fn grub_embedded_partition(image: &[u8]) -> Option<u32> {
    for tag in [&b",gpt"[..], &b",msdos"[..]] {
        let mut at = 0;
        while let Some(i) = image[at..].windows(tag.len()).position(|w| w == tag) {
            let start = at + i;
            at = start + tag.len();
            // "(" directly before, or "(hdN" before.
            let before = &image[start.saturating_sub(5)..start];
            let opened = before.last() == Some(&b'(') || before.windows(3).any(|w| w == b"(hd");
            let digits: Vec<u8> = image[at..].iter().take(4).copied().take_while(u8::is_ascii_digit).collect();
            if opened && !digits.is_empty() && image.get(at + digits.len()) == Some(&b')') {
                return core::str::from_utf8(&digits).ok()?.parse().ok();
            }
        }
    }
    None
}

fn partition_number(dp: &DevicePath) -> Option<u32> {
    dp.node_iter().find_map(|n| match n.as_enum() {
        Ok(DevicePathNodeEnum::MediaHardDrive(hd)) => Some(hd.partition_number()),
        _ => None,
    })
}

/// Whether two partitions' device paths lead to the same disk (same nodes
/// up to the partition node).
fn same_disk(a: &DevicePath, b: &DevicePath) -> bool {
    let disk = |dp: &DevicePath| -> Vec<u8> {
        dp.node_iter()
            .take_while(|n| n.device_type() != DeviceType::MEDIA)
            .flat_map(|n| n.data().iter().copied().chain([n.full_type().0.0, n.full_type().1.0]))
            .collect()
    };
    disk(a) == disk(b)
}

/// The UUID a distro's ESP `grub.cfg` stub searches for, e.g.
/// `search.fs_uuid 0b9f... root` or `search --fs-uuid --set=dev 0b9f...`.
fn grub_stub_root(text: &[u8]) -> Option<String> {
    let text = core::str::from_utf8(text).ok()?;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.first() {
            Some(&"search.fs_uuid") => return words.get(1).map(|s| s.to_string()),
            Some(&"search") if words.contains(&"--fs-uuid") || words.contains(&"-u") => {
                return words.iter().skip(1).find(|w| !w.starts_with('-') && w.len() >= 8).map(|s| s.to_string());
            }
            _ => {}
        }
    }
    None
}

/// Linux PARTUUID for a partition: the GPT partition GUID, or for MBR
/// disks "<disk signature>-<partition number>" as the kernel spells it.
fn linux_partuuid(dp: &DevicePath) -> String {
    dp.node_iter()
        .find_map(|node| match node.as_enum() {
            Ok(DevicePathNodeEnum::MediaHardDrive(hd)) => Some(match hd.partition_signature() {
                PartitionSignature::Guid(g) => format!("{g}").to_lowercase(),
                PartitionSignature::Mbr(s) => format!("{:08x}-{:02x}", u32::from_le_bytes(s), hd.partition_number()),
                _ => String::new(),
            }),
            _ => None,
        })
        .unwrap_or_default()
}

/// Linux installs Lumen can start directly. Each one takes over the card of
/// its distro's own loader on the ESP when that's clearly the same system,
/// keeping the loader as a fallback (and its id, so a remembered choice
/// still points at it).
fn add_linux(entries: &mut Vec<Entry>) {
    let mut parts = Vec::new();
    let mut where_ = Vec::new();
    for h in boot::find_handles::<BlockIO>().unwrap_or_default() {
        let Some(block) = open::<BlockIO>(h) else { continue };
        let m = block.media();
        // Partitions on fixed disks only: a USB stick stays one card.
        if !m.is_logical_partition() || m.is_removable_media() || !m.is_media_present() {
            continue;
        }
        drop(block);
        let Some(path) = open::<DevicePath>(h).map(|p| p.to_boxed()) else { continue };
        if path.node_iter().any(|n| matches!(n.as_enum(), Ok(DevicePathNodeEnum::MessagingUsb(_) | DevicePathNodeEnum::MessagingUsbClass(_)))) {
            continue;
        }
        let Some(fs) = linux_boot::open_fs(h) else {
            log::info!("partition {}: no readable Linux file system", linux_partuuid(&path));
            continue;
        };
        log::info!("partition {}: {:?} {:?} {}", linux_partuuid(&path), fs.fs_type(), fs.label(), fs.uuid());
        parts.push(linux::Part { fs, partuuid: linux_partuuid(&path) });
        where_.push((h, path));
    }
    for inst in linux::discover(&mut parts) {
        let (handle, ref path) = where_[inst.boot];
        let boot_uuid = parts[inst.boot].fs.uuid();
        log::info!("linux {:?} {} on {boot_uuid}: {} {:?} [{}]", inst.name, inst.version, inst.kernel, inst.initrds, inst.cmdline);
        let loaders: Vec<usize> = (0..entries.len())
            .filter(|&i| {
                let e = &entries[i];
                e.linux.is_none() && !e.utility && !e.nvram_only && e.file.to_lowercase().starts_with("\\efi\\")
                    && !e.file.to_lowercase().starts_with("\\efi\\microsoft")
            })
            .collect();
        let boot_part = partition_number(path);
        let by_uuid: Vec<usize> = loaders
            .iter()
            .copied()
            .filter(|&i| match &entries[i].grub_root {
                Some(GrubRoot::Uuid(u)) => u.eq_ignore_ascii_case(&boot_uuid),
                Some(GrubRoot::Partition(n)) => {
                    Some(*n) == boot_part && same_disk(&entries[i].device_path, path)
                }
                None => false,
            })
            .collect();
        let by_name: Vec<usize> = loaders
            .iter()
            .copied()
            .filter(|&i| entries[i].grub_root.is_none() || by_uuid.is_empty())
            .filter(|&i| same_os(&entries[i].title, &inst.name) || inst.os.is_some_and(|o| entries[i].icon == o.icon))
            .collect();
        let loader = match (by_uuid.as_slice(), by_name.as_slice()) {
            ([one], _) => Some(*one),
            ([], [one]) => Some(*one),
            _ => None,
        };
        let target = linux_boot::Target {
            partition: handle,
            partition_path: path.to_boxed(),
            kernel: inst.kernel.clone(),
            initrds: inst.initrds.clone(),
            cmdline: inst.cmdline.clone(),
            version: inst.version.clone(),
        };
        let icon = inst.os.map(|o| o.icon).unwrap_or(os::LINUX);
        match loader {
            Some(i) => {
                let e = &mut entries[i];
                log::info!("  takes over loader {:?} ({})", e.title, e.file);
                e.title = inst.name.clone();
                e.icon = icon;
                e.linux = Some(target);
                e.has_loader = true;
            }
            None => {
                let (location, _) = describe(path, &parts[inst.boot].fs.label(), false);
                entries.push(Entry {
                    id: inst.id(&parts),
                    title: inst.name.clone(),
                    location,
                    file: inst.kernel.clone(),
                    icon,
                    device_path: path.to_boxed(),
                    options: None,
                    utility: false,
                    boot_option: None,
                    nvram_only: false,
                    linux: Some(target),
                    has_loader: false,
                    grub_root: None,
                });
            }
        }
    }
}
