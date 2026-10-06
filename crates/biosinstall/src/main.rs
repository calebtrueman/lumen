//! lumen-bios-install: install, update, check or remove Lumen for BIOS PCs.
//!
//!   lumen-bios-install install   DISK IMAGE
//!   lumen-bios-install heal      DISK IMAGE
//!   lumen-bios-install uninstall DISK
//!   lumen-bios-install status    DISK
//!
//! DISK is a whole disk (/dev/sda, \\.\PhysicalDrive0) or a disk image
//! file. `--sectors N` gives its size where the system can't be asked
//! (Windows raw disks).

use lumen_biosinstall as bi;
use std::process::exit;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    let mut sectors = None;
    if let Some(i) = args.iter().position(|a| a == "--sectors") {
        sectors = args.get(i + 1).and_then(|n| n.parse::<u64>().ok());
        args.drain(i..(i + 2).min(args.len()));
    }
    let (Some(cmd), Some(disk)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: lumen-bios-install install|heal|uninstall|status DISK [IMAGE]");
        exit(2);
    };
    let mut d = match bi::FileDisk::open(disk, sectors.or_else(|| block_device_sectors(disk))) {
        Ok(d) => d,
        Err(e) => fail(&format!("can't open {disk}: {e}")),
    };
    let image = || {
        let path = args.get(3).unwrap_or_else(|| fail("the Lumen BIOS image is needed"));
        let bytes = std::fs::read(path).unwrap_or_else(|e| fail(&format!("can't read {path}: {e}")));
        bi::Image::parse(&bytes).unwrap_or_else(|e| fail(&e.to_string()))
    };
    let result = match cmd.as_str() {
        "install" => bi::install(&mut d, &image()).map(|r| println!("installed at sector {} (previous boot code: {})", r.base, r.previous)),
        "heal" => bi::heal(&mut d, &image()).map(|h| match h {
            bi::Healed::Fine => println!("fine"),
            bi::Healed::NotInstalled => println!("not installed"),
            bi::Healed::Restored { previous } => println!("restored (previous boot code now: {previous})"),
        }),
        "uninstall" => bi::uninstall(&mut d).map(|removed| println!("{}", if removed { "removed" } else { "not installed" })),
        "status" => bi::status(&mut d).map(|s| match s {
            bi::Status::NotInstalled => println!("not installed"),
            bi::Status::Installed { base } => println!("installed at sector {base}"),
            bi::Status::Displaced { base } => println!("displaced (area at sector {base})"),
        }),
        _ => fail("unknown command"),
    };
    if let Err(e) = result {
        fail(&e.to_string());
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("lumen-bios-install: {msg}");
    exit(1)
}

/// Block devices report size 0 through seek on some systems; ask sysfs.
fn block_device_sectors(path: &str) -> Option<u64> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    let s = std::fs::read_to_string(format!("/sys/class/block/{name}/size")).ok()?;
    s.trim().parse().ok()
}
