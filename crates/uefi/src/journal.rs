//! Lumen's log: what it found and what happened when it started something,
//! saved as `lumen.log` next to Lumen on the EFI system partition (readable
//! from Windows or Linux), so a problem on a real PC can be diagnosed from
//! one file. Test builds also send it to QEMU's debug console.

use alloc::format;
use alloc::string::String;
use core::fmt::Write;
use log::{Level, LevelFilter, Log, Metadata, Record};
use uefi::fs::{FileSystem, PathBuf};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::CString16;

struct Journal;
static JOURNAL: Journal = Journal;
static mut TEXT: String = String::new();
const MAX: usize = 192 * 1024;

fn text() -> &'static mut String {
    unsafe { &mut *(&raw mut TEXT) }
}

impl Log for Journal {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= Level::Info
    }
    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let t = text();
        if t.len() < MAX {
            let at = t.len();
            let _ = writeln!(t, "{:>5}  {}", r.level(), r.args());
            debugcon(&t[at..]);
        }
    }
    fn flush(&self) {}
}

#[cfg(all(feature = "debugcon", target_arch = "x86_64"))]
fn debugcon(s: &str) {
    for b in s.bytes() {
        unsafe { core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack)) };
    }
}
#[cfg(not(all(feature = "debugcon", target_arch = "x86_64")))]
fn debugcon(_: &str) {}

pub fn init() {
    let _ = log::set_logger(&JOURNAL);
    log::set_max_level(LevelFilter::Info);
}

/// Write the log to `<dir>\lumen.log` on Lumen's own partition.
pub fn save(device: Option<uefi::Handle>, dir: &str) {
    let Some(sfs) = device.and_then(crate::discover::open::<SimpleFileSystem>) else { return };
    let mut fs = FileSystem::new(sfs);
    let Ok(path) = CString16::try_from(format!("{dir}\\lumen.log").as_str()) else { return };
    let body = text().replace('\n', "\r\n");
    let _ = fs.write(PathBuf::from(path), body.as_bytes());
}
