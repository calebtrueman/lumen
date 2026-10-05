//! Scripted pointer devices for VM tests (QEMU's OVMF has no USB mouse
//! driver). Included by register_boot.rs when the disk has a `mouse-test`
//! marker. A relative "mouse" wiggles first; then an absolute "tablet" moves
//! to (MOUSE_X, MOUSE_Y) per mille of the screen and clicks.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};
use uefi::boot;
use uefi_raw::protocol::console::{
    AbsolutePointerMode, AbsolutePointerProtocol, AbsolutePointerState, SimplePointerMode, SimplePointerProtocol, SimplePointerState,
};
use uefi_raw::{Boolean, Status};

static REL_CALLS: AtomicU32 = AtomicU32::new(0);
static ABS_CALLS: AtomicU32 = AtomicU32::new(0);
pub static mut TARGET: (u64, u64) = (500, 500);

static REL_MODE: SimplePointerMode =
    SimplePointerMode { resolution_x: 1, resolution_y: 1, resolution_z: 1, left_button: Boolean::TRUE, right_button: Boolean::TRUE };
static ABS_MODE: AbsolutePointerMode = AbsolutePointerMode {
    absolute_min_x: 0,
    absolute_min_y: 0,
    absolute_min_z: 0,
    absolute_max_x: 1000,
    absolute_max_y: 1000,
    absolute_max_z: 0,
    attributes: uefi_raw::protocol::console::AbsolutePointerModeAttributes::empty(),
};

unsafe extern "efiapi" fn rel_reset(_: *mut SimplePointerProtocol, _: Boolean) -> Status {
    Status::SUCCESS
}
unsafe extern "efiapi" fn rel_state(_: *mut SimplePointerProtocol, s: *mut SimplePointerState) -> Status {
    let n = REL_CALLS.fetch_add(1, Ordering::Relaxed);
    if (20..40).contains(&n) {
        unsafe { *s = SimplePointerState { relative_movement_x: if n < 30 { 3 } else { -3 }, ..Default::default() } };
        Status::SUCCESS
    } else {
        Status::NOT_READY
    }
}
unsafe extern "efiapi" fn abs_reset(_: *mut AbsolutePointerProtocol, _: Boolean) -> Status {
    Status::SUCCESS
}
unsafe extern "efiapi" fn abs_state(_: *mut AbsolutePointerProtocol, s: *mut AbsolutePointerState) -> Status {
    let n = ABS_CALLS.fetch_add(1, Ordering::Relaxed);
    let (x, y) = unsafe { TARGET };
    let buttons = match n {
        0..80 => return Status::NOT_READY,
        // Hover for a while (screenshot window), then click.
        400..404 => 1,
        _ => 0,
    };
    unsafe { *s = AbsolutePointerState { current_x: x, current_y: y, current_z: 0, active_buttons: buttons } };
    Status::SUCCESS
}

static mut REL: SimplePointerProtocol = SimplePointerProtocol {
    reset: rel_reset,
    get_state: rel_state,
    wait_for_input: core::ptr::null_mut(),
    mode: &REL_MODE,
};
static mut ABS: AbsolutePointerProtocol = AbsolutePointerProtocol {
    reset: abs_reset,
    get_state: abs_state,
    wait_for_input: core::ptr::null_mut(),
    mode: &ABS_MODE,
};

pub fn install(target: (u64, u64)) {
    unsafe {
        TARGET = target;
        boot::install_protocol_interface(None, &SimplePointerProtocol::GUID, (&raw const REL).cast::<c_void>()).unwrap();
        boot::install_protocol_interface(None, &AbsolutePointerProtocol::GUID, (&raw const ABS).cast::<c_void>()).unwrap();
    }
    log::info!("installed scripted test pointers, target {target:?}");
}
