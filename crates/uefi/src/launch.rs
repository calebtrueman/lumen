//! Starting a loader, rebooting into firmware setup, and remembering the
//! last choice in an NVRAM variable.

use crate::discover::Entry;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::BootPolicy;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{cstr16, guid, CString16, Status};

const VENDOR: VariableVendor = VariableVendor(guid!("4c756d65-6e00-4b6f-9f2a-6c756d656e21"));
const OS_INDICATIONS_BOOT_TO_FW_UI: u64 = 1;

pub enum Outcome {
    /// The loader ran and returned control to us.
    Exited,
    /// The loader could not be loaded or was refused before it ran.
    Refused(Status),
}

/// Loads and starts `entry`. Only returns if the loader couldn't start or
/// exited back to us.
pub fn start(entry: &Entry) -> Outcome {
    let image = match boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath { device_path: &entry.device_path, boot_policy: BootPolicy::ExactMatch },
    ) {
        Ok(image) => image,
        Err(e) => return Outcome::Refused(e.status()),
    };
    if let Some(opts) = &entry.options {
        if let (Ok(s), Ok(mut li)) = (CString16::try_from(opts.as_str()), boot::open_protocol_exclusive::<LoadedImage>(image)) {
            // The loader may read its options at any time, so they must
            // outlive this function.
            let bytes: &'static [u16] = Box::leak(s.to_u16_slice_with_nul().to_vec().into_boxed_slice());
            unsafe { li.set_load_options(bytes.as_ptr().cast(), (bytes.len() * 2) as u32) };
        }
    }
    match boot::start_image(image) {
        Ok(()) => Outcome::Exited,
        // Deferred Secure Boot verdicts arrive from StartImage.
        Err(e) if e.status() == Status::SECURITY_VIOLATION => Outcome::Refused(e.status()),
        Err(_) => Outcome::Exited,
    }
}

pub fn remember(entry: &Entry) {
    let _ = runtime::set_variable(
        cstr16!("LumenLastBoot"),
        &VENDOR,
        VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS,
        entry.id.as_bytes(),
    );
}

/// Records that Lumen started and drew its menu on this machine. The
/// installers only make Lumen the default boot entry (and the heal tasks
/// only keep it there) once this exists, so a Lumen that can't run here
/// never gets in the way. Written only when missing to spare NVRAM.
pub fn mark_healthy() {
    let version = env!("CARGO_PKG_VERSION").as_bytes();
    let current = runtime::get_variable_boxed(cstr16!("LumenHealthy"), &VENDOR).ok();
    if current.as_ref().map(|(d, _)| &d[..]) != Some(version) {
        let _ = runtime::set_variable(
            cstr16!("LumenHealthy"),
            &VENDOR,
            VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
            version,
        );
    }
}

pub fn last_choice() -> Option<String> {
    let (data, _) = runtime::get_variable_boxed(cstr16!("LumenLastBoot"), &VENDOR).ok()?;
    String::from_utf8(data.into_vec()).ok()
}

fn read_u64(name: &uefi::CStr16) -> Option<u64> {
    let (data, _) = runtime::get_variable_boxed(name, &VariableVendor::GLOBAL_VARIABLE).ok()?;
    let mut b = [0u8; 8];
    let n = data.len().min(8);
    b[..n].copy_from_slice(&data[..n]);
    Some(u64::from_le_bytes(b))
}

pub fn firmware_setup_supported() -> bool {
    read_u64(cstr16!("OsIndicationsSupported")).is_some_and(|v| v & OS_INDICATIONS_BOOT_TO_FW_UI != 0)
}

pub fn reboot_to_firmware() -> Status {
    let current = read_u64(cstr16!("OsIndications")).unwrap_or(0);
    let r = runtime::set_variable(
        cstr16!("OsIndications"),
        &VariableVendor::GLOBAL_VARIABLE,
        VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
        &(current | OS_INDICATIONS_BOOT_TO_FW_UI).to_le_bytes(),
    );
    match r {
        Ok(()) => runtime::reset(ResetType::COLD, Status::SUCCESS, None),
        Err(e) => e.status(),
    }
}

pub fn restart() -> ! {
    runtime::reset(ResetType::COLD, Status::SUCCESS, None)
}

pub fn shutdown() -> ! {
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None)
}

/// Asks the firmware to boot its own `Boot####` entry on the next start and
/// reboots. Slower than chainloading, but it is exactly how the firmware
/// would have booted that OS by itself — immune to BitLocker PCR changes and
/// to loaders that firmware can read but we can't.
pub fn boot_next(num: u16) -> Status {
    let r = runtime::set_variable(
        cstr16!("BootNext"),
        &VariableVendor::GLOBAL_VARIABLE,
        VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
        &num.to_le_bytes(),
    );
    match r {
        Ok(()) => runtime::reset(ResetType::COLD, Status::SUCCESS, None),
        Err(e) => e.status(),
    }
}

fn boot_order() -> Vec<u16> {
    runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE)
        .map(|(d, _)| d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
        .unwrap_or_default()
}

/// Description of firmware boot entry `num`.
fn boot_description(num: u16) -> Option<String> {
    let name = CString16::try_from(alloc::format!("Boot{num:04X}").as_str()).ok()?;
    let (data, _) = runtime::get_variable_boxed(&name, &VariableVendor::GLOBAL_VARIABLE).ok()?;
    let units: Vec<u16> = data.get(6..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

/// Our own firmware entry, if this run was started from it.
fn own_entry() -> Option<u16> {
    let (cur, _) = runtime::get_variable_boxed(cstr16!("BootCurrent"), &VariableVendor::GLOBAL_VARIABLE).ok()?;
    let cur = cur.get(..2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
    boot_description(cur).is_some_and(|d| d.trim().eq_ignore_ascii_case("lumen")).then_some(cur)
}

/// Before starting an OS, make Lumen the firmware's one-shot "next boot".
///
/// Distro installers and GRUB updates (`grub-install`), and sometimes
/// Windows updates, move their own entry to the front of BootOrder while
/// that OS is running. They don't touch BootNext, so the next start still
/// lands in Lumen, which then moves itself back to first place
/// (`heal_boot_order`). Only done when we were started from our own entry.
pub fn arm_return_to_lumen() {
    let Some(num) = own_entry() else { return };
    let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    if runtime::set_variable(cstr16!("BootNext"), &VariableVendor::GLOBAL_VARIABLE, attrs, &num.to_le_bytes()).is_ok() {
        log::info!("BootNext set to Boot{num:04X} (Lumen) so the next start returns here");
    }
}

/// If we were started from our own firmware entry but something (a Windows
/// update, `grub-install`, a firmware reset) moved another entry in front
/// of it, put Lumen back first. Never touches anything else.
pub fn heal_boot_order() {
    // Started some other way (USB, an entry we don't own)? Leave it alone.
    let Some(cur) = own_entry() else { return };
    let mut order = boot_order();
    log::info!("started from Boot{cur:04X} (Lumen); BootOrder starts with {:04X?}", order.first());
    if order.first() == Some(&cur) {
        return;
    }
    order.retain(|&n| n != cur);
    order.insert(0, cur);
    let bytes: Vec<u8> = order.iter().flat_map(|n| n.to_le_bytes()).collect();
    let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    if runtime::set_variable(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE, attrs, &bytes).is_ok() {
        log::info!("moved Boot{cur:04X} (Lumen) back to the front of BootOrder");
    }
}

/// The auto-start delay chosen in Lumen's menu (seconds, -1 = off), kept in
/// NVRAM so it survives reinstalls and overrides `timeout` in lumen.conf.
pub fn saved_auto_start() -> Option<i32> {
    let (data, _) = runtime::get_variable_boxed(cstr16!("LumenAutoStart"), &VENDOR).ok()?;
    Some(i32::from_le_bytes(data.get(..4)?.try_into().ok()?))
}

pub fn save_auto_start(secs: i32) {
    let _ = runtime::set_variable(
        cstr16!("LumenAutoStart"),
        &VENDOR,
        VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
        &secs.to_le_bytes(),
    );
}

/// A one-time message for the next start (why the last start took a detour).
pub fn set_note(text: &str) {
    let current = runtime::get_variable_boxed(cstr16!("LumenNote"), &VENDOR).ok();
    if current.as_ref().map(|(d, _)| &d[..]) != Some(text.as_bytes()) {
        let _ = runtime::set_variable(
            cstr16!("LumenNote"),
            &VENDOR,
            VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS,
            text.as_bytes(),
        );
    }
}

pub fn take_note() -> Option<String> {
    let (data, _) = runtime::get_variable_boxed(cstr16!("LumenNote"), &VENDOR).ok()?;
    let _ = runtime::delete_variable(cstr16!("LumenNote"), &VENDOR);
    let text = String::from_utf8_lossy(&data).into_owned();
    log::info!("note from last start: {text}");
    (!text.is_empty()).then_some(text)
}
