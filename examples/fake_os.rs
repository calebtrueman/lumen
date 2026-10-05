//! Test stand-in for a real OS loader: shows which file was launched and
//! with what options, then returns to the boot picker.

#![no_std]
#![no_main]

use core::time::Duration;
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
    drop(img);
    boot::stall(Duration::from_secs(3));
    Status::SUCCESS
}
