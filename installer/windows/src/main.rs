//! Lumen one-click installer for Windows. Unpacks the bundled files to a
//! temporary folder and runs lumen-windows.ps1 with its graphical front end.
//! The embedded manifest makes Windows ask for administrator rights.

#![windows_subsystem = "windows"]

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

static FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/files.rs"));
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

fn error(msg: &str) -> ! {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(core::ptr::null_mut(), wide(msg).as_ptr(), wide("Lumen").as_ptr(), 0x10) };
    std::process::exit(1)
}

fn unpack() -> std::io::Result<PathBuf> {
    let dir = env::temp_dir().join(format!("Lumen-Installer-{}", std::process::id()));
    for (name, data) in FILES {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, data)?;
    }
    Ok(dir)
}

fn main() {
    let dir = unpack().unwrap_or_else(|e| error(&format!("Couldn't unpack the installer: {e}")));
    let root = env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let powershell = PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut args: Vec<String> = vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into()];
    args.push(dir.join("lumen-windows.ps1").display().to_string());
    args.push("-Bundle".into());
    args.push(dir.display().to_string());
    // `/quiet` installs silently (for scripted deployment), `/uninstall`
    // removes, `/diagnose` saves a report to the Desktop and changes nothing.
    let mut quiet = false;
    for a in env::args().skip(1) {
        match a.to_ascii_lowercase().trim_start_matches(['/', '-']) {
            "quiet" | "silent" | "s" | "q" => quiet = true,
            "uninstall" | "remove" => args.push("-Uninstall".into()),
            "diagnose" | "diag" => args.push("-Diagnose".into()),
            _ => {}
        }
    }
    args.push(if quiet { "-Yes" } else { "-Gui" }.into());
    let status = Command::new(&powershell).args(&args).creation_flags(CREATE_NO_WINDOW).status();
    let _ = fs::remove_dir_all(&dir);
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => error(&format!("Couldn't start Windows PowerShell ({}): {e}", powershell.display())),
    }
}
