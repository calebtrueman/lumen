//! Test stand-in for a real OS loader: shows which file was launched and
//! with what options, then returns to the boot picker.

#![no_std]
#![no_main]

extern crate alloc;

use core::time::Duration;
use uefi::fs::FileSystem;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::cstr16;
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{boot, println};

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

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    let _ = uefi::system::with_stdout(|o| o.clear());
    let img = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).unwrap();
    println!("FAKE OS LOADER STARTED");
    if let Some(p) = img.file_path() {
        println!("path: {}", p);
    }
    if let Ok(opts) = img.load_options_as_cstr16() {
        println!("options: {}", opts);
    }
    let device = img.device();
    drop(img);

    // VM test of Lumen's "stay default" (tools/vm.py --grub-takeover): act
    // like a distro installer / grub-install, which moves its own entry to
    // the front of BootOrder, then reboot. Once per run.
    let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    let vendor = VariableVendor(uefi::guid!("4c756d65-6e00-4b6f-9f2a-6c756d656e21"));
    let marker = device.and_then(|d| {
        let params = boot::OpenProtocolParams { handle: d, agent: boot::image_handle(), controller: None };
        let sfs = unsafe { boot::open_protocol::<SimpleFileSystem>(params, boot::OpenProtocolAttributes::GetProtocol) }.ok()?;
        FileSystem::new(sfs).try_exists(cstr16!("\\grub-takeover")).ok()
    }) == Some(true);
    if marker && runtime::get_variable_boxed(cstr16!("FakeTakeoverDone"), &vendor).is_err() {
        let _ = runtime::set_variable(cstr16!("FakeTakeoverDone"), &vendor, attrs, &[1]);
        if let Ok((order, _)) = runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE) {
            let mut o = order.to_vec();
            o.rotate_left(2); // the entry that was first is now last
            let _ = runtime::set_variable(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE, attrs, &o);
            log::info!("fake grub-install: BootOrder now {:02x?}; rebooting", o);
        }
        runtime::reset(ResetType::COLD, uefi::Status::SUCCESS, None);
    }
    boot::stall(Duration::from_secs(3));
    Status::SUCCESS
}
