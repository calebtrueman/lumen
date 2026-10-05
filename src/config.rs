//! `lumen.conf`, read from the directory Lumen itself was loaded from.
//!
//! ```text
//! timeout 5              # seconds; 0 boots the default at once, -1 waits forever
//! default last           # "last" (remember previous choice) or part of a title
//! hide Recovery          # hide entries whose title or path contains this
//! resolution max         # "keep", "max" or WIDTHxHEIGHT
//! clock off              # hide the clock
//! bootnext Windows       # start matching entries via the firmware's own
//!                        # boot entry + reboot (BitLocker-safe)
//! entry Arch (fallback) | \EFI\arch\vmlinuz-linux.efi | initrd=\initramfs-linux-fallback.img
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub enum Resolution {
    Keep,
    Max,
    Exact(usize, usize),
}

pub struct CustomEntry {
    pub title: String,
    pub path: String,
    pub options: Option<String>,
}

pub struct Config {
    pub timeout: i32,
    pub default: String,
    pub hide: Vec<String>,
    pub resolution: Resolution,
    pub clock: bool,
    pub bootnext: Vec<String>,
    pub entries: Vec<CustomEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: 5,
            default: "last".into(),
            hide: Vec::new(),
            resolution: Resolution::Keep,
            clock: true,
            bootnext: Vec::new(),
            entries: Vec::new(),
        }
    }
}

impl Config {
    pub fn parse(src: &str) -> Self {
        let mut cfg = Config::default();
        for line in src.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let (key, val) = match line.split_once(char::is_whitespace) {
                Some((k, v)) => (k, v.trim()),
                None => (line, ""),
            };
            match key {
                "timeout" => cfg.timeout = val.parse().unwrap_or(cfg.timeout),
                "default" => cfg.default = val.to_string(),
                "hide" if !val.is_empty() => cfg.hide.push(val.to_lowercase()),
                "bootnext" if !val.is_empty() => cfg.bootnext.push(val.to_lowercase()),
                "clock" => cfg.clock = !matches!(val, "off" | "no" | "false" | "0"),
                "resolution" => {
                    cfg.resolution = match val {
                        "max" => Resolution::Max,
                        "keep" => Resolution::Keep,
                        v => v
                            .split_once('x')
                            .and_then(|(w, h)| Some(Resolution::Exact(w.parse().ok()?, h.parse().ok()?)))
                            .unwrap_or(Resolution::Keep),
                    }
                }
                "entry" => {
                    let mut parts = val.splitn(3, '|').map(str::trim);
                    if let (Some(title), Some(path)) = (parts.next(), parts.next()) {
                        cfg.entries.push(CustomEntry {
                            title: title.into(),
                            path: path.replace('/', "\\"),
                            options: parts.next().filter(|o| !o.is_empty()).map(Into::into),
                        });
                    }
                }
                _ => {}
            }
        }
        cfg
    }
}
