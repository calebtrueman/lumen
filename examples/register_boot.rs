//! Test helper: registers firmware boot entries the way an OS installer
//! would (one valid entry at an unusual path, one stale entry for a
//! partition that doesn't exist), then chainloads \EFI\lumen\lumen.efi.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource};
use uefi::fs::FileSystem;
use uefi::prelude::*;
use uefi::proto::BootPolicy;
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::media::{PartitionFormat, PartitionSignature};
use uefi::proto::device_path::{DevicePath, DevicePathNodeEnum};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{CStr16, Guid, cstr16};

#[path = "support/fake_mouse.rs"]
mod fake_mouse;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    uefi::println!("panic: {info}");
    loop {}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

fn load_option(desc: &CStr16, path: &DevicePath) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    v.extend_from_slice(&(path.as_bytes().len() as u16).to_le_bytes());
    for c in desc.to_u16_slice_with_nul() {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v.extend_from_slice(path.as_bytes());
    v
}

fn short_form(hd: build::media::HardDrive, file: &CStr16) -> alloc::boxed::Box<DevicePath> {
    let mut buf = Vec::new();
    DevicePathBuilder::with_vec(&mut buf)
        .push(&hd)
        .unwrap()
        .push(&build::media::FilePath { path_name: file })
        .unwrap()
        .finalize()
        .unwrap()
        .to_boxed()
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    let odd = cstr16!("\\EFI\\Custom\\Odd\\boot.efi");
    // Like an OS installer running later, with every disk connected.
    for h in boot::locate_handle_buffer(boot::SearchType::AllHandles).unwrap().iter() {
        let _ = boot::connect_controller(*h, &[], None, true);
    }

    for h in boot::find_handles::<SimpleFileSystem>().unwrap() {
        let params = || boot::OpenProtocolParams { handle: h, agent: boot::image_handle(), controller: None };
        let get = || boot::OpenProtocolAttributes::GetProtocol;
        let dp = unsafe { boot::open_protocol::<DevicePath>(params(), get()) }.unwrap().to_boxed();
        let mut fs = FileSystem::new(unsafe { boot::open_protocol::<SimpleFileSystem>(params(), get()) }.unwrap());
        if !fs.try_exists(odd).unwrap_or(false) {
            continue;
        }
        for node in dp.node_iter() {
            if let Ok(DevicePathNodeEnum::MediaHardDrive(hd)) = node.as_enum() {
                let real = build::media::HardDrive {
                    partition_number: hd.partition_number(),
                    partition_start: hd.partition_start(),
                    partition_size: hd.partition_size(),
                    partition_signature: hd.partition_signature(),
                    partition_format: hd.partition_format(),
                };
                let path = short_form(real, odd);
                log::info!("registering Boot0100 -> {}", path);
                let opt = load_option(cstr16!("Haiku"), &path);
                runtime::set_variable(cstr16!("Boot0100"), &VariableVendor::GLOBAL_VARIABLE, attrs, &opt).unwrap();
            }
        }
    }
    let stale = build::media::HardDrive {
        partition_number: 3,
        partition_start: 2048,
        partition_size: 1 << 20,
        partition_signature: PartitionSignature::Guid(Guid::from_bytes([7; 16])),
        partition_format: PartitionFormat::GPT,
    };
    let opt = load_option(cstr16!("Old Deleted Linux"), &short_form(stale, cstr16!("\\EFI\\gone\\grubx64.efi")));
    runtime::set_variable(cstr16!("Boot0101"), &VariableVendor::GLOBAL_VARIABLE, attrs, &opt).unwrap();

    let (order, _) = runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE).unwrap();
    let mut order = order.to_vec();
    for n in [0x0100u16, 0x0101] {
        if !order.chunks_exact(2).any(|c| u16::from_le_bytes([c[0], c[1]]) == n) {
            order.extend_from_slice(&n.to_le_bytes());
        }
    }
    runtime::set_variable(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE, attrs, &order).unwrap();

    // Simulate "a Windows update pushed Lumen down": on the first run create a
    // "Lumen" entry LAST in BootOrder and boot it once via BootNext. Lumen
    // should then move itself back to the front.
    // Opt-in (tools/vm.py --heal-test): OVMF prunes unmatched boot entries on
    // reboot, which would also delete the Boot0100 test entry above.
    let heal_test = {
        let me = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).unwrap();
        let dev = me.device().unwrap();
        drop(me);
        let params = boot::OpenProtocolParams { handle: dev, agent: boot::image_handle(), controller: None };
        let sfs = unsafe { boot::open_protocol::<SimpleFileSystem>(params, boot::OpenProtocolAttributes::GetProtocol) }.unwrap();
        FileSystem::new(sfs).try_exists(cstr16!("\\heal-test")).unwrap_or(false)
    };
    // Opt-in (tools/vm.py --mouse-test X Y): scripted pointer devices.
    {
        let me = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).unwrap();
        let dev = me.device().unwrap();
        drop(me);
        let params = boot::OpenProtocolParams { handle: dev, agent: boot::image_handle(), controller: None };
        let sfs = unsafe { boot::open_protocol::<SimpleFileSystem>(params, boot::OpenProtocolAttributes::GetProtocol) }.unwrap();
        if let Ok(t) = FileSystem::new(sfs).read_to_string(cstr16!("\\mouse-test")) {
            let mut it = t.split_whitespace().filter_map(|v| v.parse::<u64>().ok());
            fake_mouse::install((it.next().unwrap_or(500), it.next().unwrap_or(500)));
        }
    }
    let lumen_num = 0x0200u16;
    if heal_test && runtime::get_variable_boxed(cstr16!("Boot0200"), &VariableVendor::GLOBAL_VARIABLE).is_err() {
        let me = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).unwrap();
        let dev = boot::open_protocol_exclusive::<DevicePath>(me.device().unwrap()).unwrap().to_boxed();
        drop(me);
        let mut buf = Vec::new();
        let mut b = DevicePathBuilder::with_vec(&mut buf);
        for n in dev.node_iter() {
            b = b.push(&n).unwrap();
        }
        let path = b.push(&build::media::FilePath { path_name: cstr16!("\\EFI\\lumen\\lumen.efi") }).unwrap().finalize().unwrap();
        runtime::set_variable(cstr16!("Boot0200"), &VariableVendor::GLOBAL_VARIABLE, attrs, &load_option(cstr16!("Lumen"), path)).unwrap();
        let (order, _) = runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE).unwrap();
        let mut order = order.to_vec();
        order.extend_from_slice(&lumen_num.to_le_bytes());
        runtime::set_variable(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE, attrs, &order).unwrap();
        runtime::set_variable(cstr16!("BootNext"), &VariableVendor::GLOBAL_VARIABLE, attrs, &lumen_num.to_le_bytes()).unwrap();
        log::info!("created Boot0200 'Lumen' last in BootOrder; rebooting into it");
        runtime::reset(runtime::ResetType::COLD, Status::SUCCESS, None);
    }

    // Chainload Lumen from this disk.
    let me = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).unwrap();
    let dev = boot::open_protocol_exclusive::<DevicePath>(me.device().unwrap()).unwrap().to_boxed();
    drop(me);
    let mut buf = Vec::new();
    let mut b = DevicePathBuilder::with_vec(&mut buf);
    for n in dev.node_iter() {
        b = b.push(&n).unwrap();
    }
    let path = b.push(&build::media::FilePath { path_name: cstr16!("\\EFI\\lumen\\lumen.efi") }).unwrap().finalize().unwrap();
    let img = boot::load_image(boot::image_handle(), LoadImageSource::FromDevicePath { device_path: path, boot_policy: BootPolicy::ExactMatch }).unwrap();
    boot::start_image(img).status()
}
