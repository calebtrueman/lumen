//! Starting Linux directly: Lumen reads the kernel and initrd from the
//! distro's own file system and runs the kernel's EFI stub itself, the way
//! systemd-boot does, so GRUB isn't involved.
//!
//! - The command line goes in the kernel image's LoadOptions.
//! - The initrd is offered through the LINUX_EFI_INITRD_MEDIA LoadFile2
//!   protocol, which the EFI stub of Linux 5.8+ asks for.
//! - Under Secure Boot the kernel must pass shim's verification (the distro
//!   or MOK keys shim trusts), exactly as it must when GRUB loads it. Only
//!   that one verified image is then let through the firmware's own check,
//!   which knows nothing of shim's keys. Anything shim rejects is refused
//!   and Lumen falls back to the distro's own loader.

use crate::discover::open;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use lumen_core::fs::{self, FileSystem, Volume};
use lumen_core::linux;
use uefi::boot::{self, LoadImageSource};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::disk::DiskIo;
use uefi::runtime::{self, VariableVendor};
use uefi::{cstr16, guid, CString16, Guid, Handle, Status};
use uefi_raw::protocol::device_path::DevicePathProtocol;
use uefi_raw::protocol::media::LoadFile2Protocol;

/// A partition read through the firmware's DiskIo protocol.
pub struct DiskVolume {
    io: boot::ScopedProtocol<DiskIo>,
    media_id: u32,
    size: u64,
}

impl DiskVolume {
    pub fn open(handle: Handle) -> Option<Self> {
        let block = open::<BlockIO>(handle)?;
        let media = block.media();
        if !media.is_media_present() {
            return None;
        }
        let size = (media.last_block() + 1) * media.block_size() as u64;
        let media_id = media.media_id();
        drop(block);
        Some(Self { io: open::<DiskIo>(handle)?, media_id, size })
    }
}

impl Volume for DiskVolume {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> fs::Result<()> {
        self.io.read_disk(self.media_id, offset, buf).map_err(|_| fs::Error::Io)
    }
    fn size(&self) -> u64 {
        self.size
    }
}

pub fn open_fs(handle: Handle) -> Option<Box<dyn FileSystem>> {
    fs::open(Box::new(DiskVolume::open(handle)?))
}

/// What to start, recorded at scan time.
pub struct Target {
    /// The partition holding the kernel and initrds.
    pub partition: Handle,
    pub partition_path: Box<DevicePath>,
    pub kernel: String,
    pub initrds: Vec<String>,
    pub cmdline: String,
    pub version: String,
}

pub fn secure_boot_enabled() -> bool {
    matches!(runtime::get_variable_boxed(cstr16!("SecureBoot"), &VariableVendor::GLOBAL_VARIABLE), Ok((v, _)) if v.first() == Some(&1))
}

/// Load and start the kernel. Returns only if it couldn't be started (or
/// the kernel itself returned), with a reason for the user.
pub fn boot(target: &Target) -> String {
    let Some(mut fsys) = open_fs(target.partition) else {
        return String::from("its file system can no longer be read");
    };
    let kernel = match fs::read_file(fsys.as_mut(), &target.kernel, fs::IMAGE_LIMIT) {
        Ok(k) => k,
        Err(e) => return format!("couldn't read the kernel ({e:?})"),
    };
    if !is_pe(&kernel) {
        return String::from("the kernel has no EFI stub");
    }
    if !target.initrds.is_empty() && !linux::supports_initrd_loadfile2(&kernel) {
        return String::from("the kernel is older than Linux 5.8");
    }
    let secure = secure_boot_enabled();
    if secure {
        if let Err(why) = shim_verify(&kernel) {
            return why;
        }
    }
    // The initrds, concatenated, each padded to 4 bytes as GRUB does (cpio
    // archives must stay aligned).
    let mut initrd = Vec::new();
    for p in &target.initrds {
        match fs::read_file(fsys.as_mut(), p, fs::IMAGE_LIMIT) {
            Ok(data) => {
                initrd.extend_from_slice(&data);
                initrd.resize(initrd.len().next_multiple_of(4), 0);
            }
            Err(e) => return format!("couldn't read {p} ({e:?})"),
        }
    }
    drop(fsys);

    let file_path = crate::discover::file_device_path(&target.partition_path, &target.kernel.replace('/', "\\"));
    let image = {
        let _pass = secure.then(|| SecurityPass::install(&kernel));
        boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromBuffer { buffer: &kernel, file_path: Some(&file_path) },
        )
    };
    let image = match image {
        Ok(i) => i,
        Err(e) => return format!("the firmware refused the kernel ({:?})", e.status()),
    };
    drop(kernel);

    let cmdline = CString16::try_from(target.cmdline.as_str()).unwrap_or_default();
    let options: &'static [u16] = Box::leak(cmdline.to_u16_slice_with_nul().to_vec().into_boxed_slice());
    match boot::open_protocol_exclusive::<LoadedImage>(image) {
        Ok(mut li) => unsafe { li.set_load_options(options.as_ptr().cast(), (options.len() * 2) as u32) },
        Err(_) => {
            let _ = boot::unload_image(image);
            return String::from("couldn't pass the kernel its command line");
        }
    }

    let initrd_handle = if initrd.is_empty() { None } else { InitrdProvider::install(initrd) };
    if !target.initrds.is_empty() && initrd_handle.is_none() {
        let _ = boot::unload_image(image);
        return String::from("couldn't hand over the initrd");
    }
    let result = boot::start_image(image);
    // Only reached if the kernel failed to start (or returned).
    if let Some(h) = initrd_handle {
        InitrdProvider::uninstall(h);
    }
    match result {
        Ok(()) => String::from("the kernel returned"),
        Err(e) => format!("the kernel stopped ({:?})", e.status()),
    }
}

fn is_pe(image: &[u8]) -> bool {
    if image.len() < 0x40 || &image[..2] != b"MZ" {
        return false;
    }
    let pe = u32::from_le_bytes(image[0x3C..0x40].try_into().unwrap()) as usize;
    image.get(pe..pe + 4) == Some(b"PE\0\0")
}

// ---------------------------------------------------------------------------
// Secure Boot: verification through shim

const SHIM_LOCK_GUID: Guid = guid!("605dab50-e046-4300-abb6-3dd810dd8b23");

/// shim declares its protocol functions without EFIAPI, so on x86_64 they
/// use the System V calling convention (systemd-boot calls them the same
/// way); on AArch64 there's only one convention.
#[cfg(target_arch = "x86_64")]
type ShimVerify = unsafe extern "sysv64" fn(buffer: *const c_void, size: u32) -> Status;
#[cfg(not(target_arch = "x86_64"))]
type ShimVerify = unsafe extern "C" fn(buffer: *const c_void, size: u32) -> Status;

#[repr(C)]
struct ShimLock {
    verify: ShimVerify,
    hash: *const c_void,
    context: *const c_void,
}

fn locate<T>(guid: &Guid) -> Option<*mut T> {
    let st = uefi::table::system_table_raw()?;
    let bs = unsafe { (*st.as_ptr()).boot_services };
    let mut out: *mut c_void = ptr::null_mut();
    let status = unsafe { ((*bs).locate_protocol)(guid, ptr::null_mut(), &mut out) };
    (status == Status::SUCCESS && !out.is_null()).then_some(out.cast())
}

fn shim_verify(image: &[u8]) -> Result<(), String> {
    let Some(shim) = locate::<ShimLock>(&SHIM_LOCK_GUID) else {
        return Err(String::from("Secure Boot is on and shim isn't available to check the kernel"));
    };
    let Ok(size) = u32::try_from(image.len()) else { return Err(String::from("the kernel is too large")) };
    match unsafe { ((*shim).verify)(image.as_ptr().cast(), size) } {
        Status::SUCCESS => Ok(()),
        s => Err(format!("Secure Boot: the kernel isn't signed by a key this PC trusts ({s:?})")),
    }
}

const SECURITY_ARCH_GUID: Guid = guid!("a46423e3-4617-49f1-b9ff-d1bfa9115839");
const SECURITY2_ARCH_GUID: Guid = guid!("94ab2f58-1438-4ef1-9152-18941a3a0e68");

#[repr(C)]
struct SecurityArch {
    file_authentication_state:
        unsafe extern "efiapi" fn(this: *const SecurityArch, auth_status: u32, file: *const DevicePathProtocol) -> Status,
}

#[repr(C)]
struct Security2Arch {
    file_authentication: unsafe extern "efiapi" fn(
        this: *const Security2Arch,
        file: *const DevicePathProtocol,
        buffer: *mut c_void,
        size: usize,
        boot_policy: bool,
    ) -> Status,
}

// The image shim has verified, and the firmware checks being wrapped.
static mut VERIFIED: (usize, usize) = (0, 0);
static mut ORIGINAL1: Option<unsafe extern "efiapi" fn(*const SecurityArch, u32, *const DevicePathProtocol) -> Status> = None;
static mut ORIGINAL2: Option<
    unsafe extern "efiapi" fn(*const Security2Arch, *const DevicePathProtocol, *mut c_void, usize, bool) -> Status,
> = None;

/// While alive, the firmware's image check accepts exactly the buffer that
/// shim verified (same address and size) and judges everything else as
/// before. The firmware's db doesn't hold distro or MOK keys; shim does.
struct SecurityPass {
    sec1: Option<*mut SecurityArch>,
    sec2: Option<*mut Security2Arch>,
}

unsafe extern "efiapi" fn check2(
    this: *const Security2Arch,
    file: *const DevicePathProtocol,
    buffer: *mut c_void,
    size: usize,
    boot_policy: bool,
) -> Status {
    unsafe {
        if !buffer.is_null() && (buffer as usize, size) == VERIFIED {
            return Status::SUCCESS;
        }
        match ORIGINAL2 {
            Some(f) => f(this, file, buffer, size, boot_policy),
            None => Status::SECURITY_VIOLATION,
        }
    }
}

unsafe extern "efiapi" fn check1(this: *const SecurityArch, auth_status: u32, file: *const DevicePathProtocol) -> Status {
    // The older protocol doesn't see the buffer. Firmware with Security2
    // only consults it for firmware-volume images; without Security2 it's
    // the one check, and Lumen's verified load is the only one in flight
    // while the pass exists.
    unsafe {
        let (verified, original2) = (VERIFIED, ORIGINAL2);
        if verified.0 != 0 && original2.is_none() {
            return Status::SUCCESS;
        }
        match ORIGINAL1 {
            Some(f) => f(this, auth_status, file),
            None => Status::SECURITY_VIOLATION,
        }
    }
}

impl SecurityPass {
    fn install(image: &[u8]) -> Self {
        let sec1 = locate::<SecurityArch>(&SECURITY_ARCH_GUID);
        let sec2 = locate::<Security2Arch>(&SECURITY2_ARCH_GUID);
        unsafe {
            VERIFIED = (image.as_ptr() as usize, image.len());
            if let Some(p) = sec2 {
                ORIGINAL2 = Some((*p).file_authentication);
                (*p).file_authentication = check2;
            }
            if let Some(p) = sec1 {
                ORIGINAL1 = Some((*p).file_authentication_state);
                (*p).file_authentication_state = check1;
            }
        }
        Self { sec1, sec2 }
    }
}

impl Drop for SecurityPass {
    fn drop(&mut self) {
        unsafe {
            if let (Some(p), Some(f)) = (self.sec2, ORIGINAL2) {
                (*p).file_authentication = f;
            }
            if let (Some(p), Some(f)) = (self.sec1, ORIGINAL1) {
                (*p).file_authentication_state = f;
            }
            VERIFIED = (0, 0);
            ORIGINAL1 = None;
            ORIGINAL2 = None;
        }
    }
}

// ---------------------------------------------------------------------------
// initrd via LoadFile2

/// The vendor media device path Linux's EFI stub looks for:
/// LINUX_EFI_INITRD_MEDIA_GUID 5568e427-68fc-4f3d-ac74-ca555231cc68.
static INITRD_DEVICE_PATH: [u8; 24] = [
    0x04, 0x03, 20, 0, // media, vendor, length 20
    0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, 0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68, //
    0x7f, 0xff, 4, 0, // end of path
];

#[repr(C)]
struct InitrdProvider {
    proto: LoadFile2Protocol,
    data: Vec<u8>,
}

unsafe extern "efiapi" fn load_initrd(
    this: *mut LoadFile2Protocol,
    _file: *const DevicePathProtocol,
    boot_policy: uefi_raw::Boolean,
    size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    unsafe {
        if this.is_null() || size.is_null() {
            return Status::INVALID_PARAMETER;
        }
        if bool::from(boot_policy) {
            return Status::UNSUPPORTED;
        }
        let data = &(*this.cast::<InitrdProvider>()).data;
        if buffer.is_null() || *size < data.len() {
            *size = data.len();
            return Status::BUFFER_TOO_SMALL;
        }
        ptr::copy_nonoverlapping(data.as_ptr(), buffer.cast(), data.len());
        *size = data.len();
        Status::SUCCESS
    }
}

impl InitrdProvider {
    fn install(data: Vec<u8>) -> Option<(Handle, *mut InitrdProvider)> {
        let provider: *mut InitrdProvider =
            Box::into_raw(Box::new(InitrdProvider { proto: LoadFile2Protocol { load_file: load_initrd }, data }));
        unsafe {
            let path = INITRD_DEVICE_PATH.as_ptr().cast();
            let Ok(h) = boot::install_protocol_interface(None, &DevicePathProtocol::GUID, path) else {
                drop(Box::from_raw(provider));
                return None;
            };
            if boot::install_protocol_interface(Some(h), &LoadFile2Protocol::GUID, provider.cast()).is_err() {
                let _ = boot::uninstall_protocol_interface(h, &DevicePathProtocol::GUID, path);
                drop(Box::from_raw(provider));
                return None;
            }
            Some((h, provider))
        }
    }

    fn uninstall((h, provider): (Handle, *mut InitrdProvider)) {
        unsafe {
            let _ = boot::uninstall_protocol_interface(h, &LoadFile2Protocol::GUID, provider.cast());
            let _ = boot::uninstall_protocol_interface(h, &DevicePathProtocol::GUID, INITRD_DEVICE_PATH.as_ptr().cast());
            drop(Box::from_raw(provider));
        }
    }
}
