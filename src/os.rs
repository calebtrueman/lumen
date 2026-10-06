//! Everything Lumen knows about operating systems: display names, icon
//! tiles in each project's brand colours, and the clues used to recognise
//! them from folder names, volume labels, boot menu titles and the signing
//! certificates embedded in their loaders.
//!
//! Logos come from Font Logos (public domain) and Font Awesome Free Brands
//! (CC BY 4.0). They identify the OS on its own boot entry; all trademarks
//! belong to their owners.

use crate::gfx::{rgb, Color};
use crate::icons::{Glyph, Icon};

pub struct Os {
    pub name: &'static str,
    /// Lower-case substrings matched against `\EFI\<dir>` names, volume
    /// labels and firmware entry descriptions. Longest match wins.
    pub keys: &'static [&'static str],
    pub icon: Icon,
}

const W: Color = rgb(255, 255, 255);

const fn logo(c: char, top: Color, bottom: Color) -> Icon {
    Icon { glyph: Glyph::Logo(c), top, bottom, ink: W, glow: None }
}
const fn brand(c: char, top: Color, bottom: Color) -> Icon {
    Icon { glyph: Glyph::Brand(c), top, bottom, ink: W, glow: None }
}
const fn letter(c: char, top: Color, bottom: Color) -> Icon {
    Icon { glyph: Glyph::Letter(c), top, bottom, ink: W, glow: None }
}
const fn os(name: &'static str, keys: &'static [&'static str], icon: Icon) -> Os {
    Os { name, keys, icon }
}

pub const WINDOWS: Icon = Icon { glyph: Glyph::Windows, top: rgb(0, 164, 239), bottom: rgb(0, 92, 190), ink: W, glow: None };
/// Tux by Larry Ewing (lewing@isc.tamu.edu) and The GIMP; SVG by Simon Budig
/// and Garrett LeSage. Rendered to 216x256 premultiplied RGBA.
pub static TUX_IMAGE: crate::gfx::Sprite =
    crate::gfx::Sprite { w: 216, h: 256, rgba: include_bytes!("../assets/tux.rgba") };

/// Full-colour Tux on a dark slate tile, glowing in his beak-and-feet yellow.
pub const LINUX: Icon =
    Icon { glyph: Glyph::Image(&TUX_IMAGE), top: rgb(70, 76, 96), bottom: rgb(28, 31, 42), ink: W, glow: Some(rgb(245, 180, 40)) };
pub const MACOS: Icon = Icon { glyph: Glyph::Brand('\u{f179}'), top: rgb(246, 246, 248), bottom: rgb(196, 198, 206), ink: rgb(28, 28, 32), glow: None };
pub const SHELL: Icon = Icon { glyph: Glyph::Shell, top: rgb(60, 66, 80), bottom: rgb(22, 24, 32), ink: W, glow: None };
pub const DRIVE: Icon = Icon { glyph: Glyph::Drive, top: rgb(110, 150, 190), bottom: rgb(50, 80, 120), ink: W, glow: None };
pub const GEAR: Icon = Icon { glyph: Glyph::Gear, top: rgb(120, 128, 145), bottom: rgb(58, 64, 80), ink: W, glow: None };

pub static ALL: &[Os] = &[
    // ---- Windows & Apple ------------------------------------------------
    os("Windows", &["windows", "microsoft"], WINDOWS),
    os("macOS", &["macos", "mac os", "macintosh", "os x"], MACOS),
    // ---- Ubuntu family ----------------------------------------------------
    os("Ubuntu", &["ubuntu"], logo('\u{f31b}', rgb(240, 100, 50), rgb(200, 60, 20))),
    os("Kubuntu", &["kubuntu"], logo('\u{f333}', rgb(0, 140, 210), rgb(0, 90, 160))),
    os("Xubuntu", &["xubuntu"], logo('\u{f31b}', rgb(40, 140, 220), rgb(20, 80, 160))),
    os("Lubuntu", &["lubuntu"], logo('\u{f31b}', rgb(0, 120, 200), rgb(0, 70, 140))),
    os("Ubuntu MATE", &["ubuntu mate", "ubuntu-mate"], logo('\u{f31b}', rgb(135, 190, 80), rgb(80, 140, 40))),
    os("KDE neon", &["kde neon", "kdeneon"], logo('\u{f331}', rgb(40, 180, 160), rgb(20, 110, 110))),
    os("Linux Mint", &["linuxmint", "linux mint", "mint"], logo('\u{f30e}', rgb(140, 210, 70), rgb(70, 150, 40))),
    os("Pop!_OS", &["pop!_os", "pop_os", "pop-os", "popos"], logo('\u{f32a}', rgb(72, 185, 199), rgb(36, 110, 125))),
    os("elementary OS", &["elementary"], logo('\u{f309}', rgb(100, 186, 255), rgb(40, 110, 200))),
    os("Zorin OS", &["zorin"], logo('\u{f32f}', rgb(21, 166, 240), rgb(10, 90, 170))),
    os("LXLE", &["lxle"], logo('\u{f33e}', rgb(110, 130, 150), rgb(60, 70, 90))),
    // ---- Debian family -----------------------------------------------------
    os("Debian", &["debian"], logo('\u{f306}', rgb(230, 30, 90), rgb(160, 0, 48))),
    os("Devuan", &["devuan"], logo('\u{f307}', rgb(90, 110, 140), rgb(40, 50, 70))),
    os("MX Linux", &["mx linux", "mx-linux", "mxlinux"], logo('\u{f33f}', rgb(70, 90, 120), rgb(30, 40, 60))),
    os("Kali Linux", &["kali"], logo('\u{f327}', rgb(54, 123, 240), rgb(20, 60, 160))),
    os("Parrot OS", &["parrot"], logo('\u{f329}', rgb(25, 190, 220), rgb(10, 100, 140))),
    os("Deepin", &["deepin"], logo('\u{f321}', rgb(15, 140, 240), rgb(10, 80, 170))),
    os("Tails", &["tails"], logo('\u{f343}', rgb(120, 70, 170), rgb(70, 35, 110))),
    os("Trisquel", &["trisquel"], logo('\u{f344}', rgb(30, 110, 200), rgb(15, 60, 130))),
    os("Puppy Linux", &["puppy"], logo('\u{f341}', rgb(120, 120, 130), rgb(60, 60, 70))),
    os("Raspberry Pi OS", &["raspberry", "raspios"], logo('\u{f315}', rgb(215, 40, 90), rgb(150, 20, 60))),
    // ---- Red Hat family -----------------------------------------------------
    os("Fedora", &["fedora"], logo('\u{f30a}', rgb(81, 162, 218), rgb(41, 65, 114))),
    os("Nobara", &["nobara"], logo('\u{f380}', rgb(110, 80, 230), rgb(50, 30, 140))),
    os("Bazzite", &["bazzite"], letter('B', rgb(140, 100, 255), rgb(70, 40, 170))),
    os("Red Hat Enterprise Linux", &["redhat", "red hat", "rhel"], logo('\u{f316}', rgb(238, 40, 40), rgb(150, 0, 0))),
    os("CentOS", &["centos"], logo('\u{f304}', rgb(160, 80, 160), rgb(70, 40, 120))),
    os("Rocky Linux", &["rocky"], logo('\u{f32b}', rgb(16, 185, 129), rgb(6, 110, 80))),
    os("AlmaLinux", &["almalinux", "alma linux"], logo('\u{f31d}', rgb(255, 160, 60), rgb(220, 60, 60))),
    os("Oracle Linux", &["oracle"], letter('O', rgb(230, 60, 40), rgb(160, 20, 10))),
    os("Mageia", &["mageia"], logo('\u{f310}', rgb(40, 140, 200), rgb(20, 80, 140))),
    os("Mandriva", &["mandriva", "openmandriva"], logo('\u{f311}', rgb(60, 130, 200), rgb(30, 70, 140))),
    // ---- SUSE --------------------------------------------------------------
    os("openSUSE", &["opensuse", "suse"], logo('\u{f314}', rgb(115, 186, 37), rgb(40, 130, 90))),
    os("openSUSE Tumbleweed", &["tumbleweed"], logo('\u{f37d}', rgb(115, 186, 37), rgb(40, 130, 90))),
    os("openSUSE Leap", &["opensuse-leap", "opensuse leap", "leap"], logo('\u{f37e}', rgb(115, 186, 37), rgb(40, 130, 90))),
    os("SUSE Linux Enterprise", &["sles", "suse linux enterprise"], brand('\u{f7d6}', rgb(48, 186, 120), rgb(10, 100, 70))),
    // ---- Arch family -------------------------------------------------------
    os("Arch Linux", &["arch", "archlinux", "archiso"], logo('\u{f303}', rgb(40, 170, 230), rgb(14, 100, 160))),
    os("Manjaro", &["manjaro"], logo('\u{f312}', rgb(53, 191, 92), rgb(25, 130, 60))),
    os("EndeavourOS", &["endeavour"], logo('\u{f322}', rgb(160, 90, 220), rgb(90, 40, 160))),
    os("Garuda Linux", &["garuda"], logo('\u{f337}', rgb(180, 120, 255), rgb(90, 50, 200))),
    os("CachyOS", &["cachyos", "cachy"], logo('\u{f385}', rgb(0, 200, 170), rgb(0, 120, 110))),
    os("ArcoLinux", &["arcolinux"], logo('\u{f346}', rgb(50, 140, 230), rgb(20, 70, 150))),
    os("Artix Linux", &["artix"], logo('\u{f31f}', rgb(16, 160, 205), rgb(10, 90, 130))),
    os("Archcraft", &["archcraft"], logo('\u{f345}', rgb(90, 110, 140), rgb(40, 50, 70))),
    os("ArchLabs", &["archlabs"], logo('\u{f31e}', rgb(90, 160, 220), rgb(40, 90, 150))),
    os("XeroLinux", &["xerolinux"], logo('\u{f34a}', rgb(120, 90, 220), rgb(60, 40, 140))),
    os("BigLinux", &["biglinux"], logo('\u{f347}', rgb(30, 140, 230), rgb(15, 80, 160))),
    os("Crystal Linux", &["crystal"], logo('\u{f348}', rgb(160, 110, 230), rgb(90, 60, 160))),
    os("Hyperbola", &["hyperbola"], logo('\u{f33a}', rgb(110, 110, 120), rgb(50, 50, 60))),
    os("Parabola", &["parabola"], logo('\u{f340}', rgb(120, 90, 200), rgb(60, 40, 120))),
    // ---- Independent Linux --------------------------------------------------
    os("NixOS", &["nixos"], logo('\u{f313}', rgb(126, 186, 228), rgb(82, 119, 195))),
    os("Gentoo", &["gentoo"], logo('\u{f30d}', rgb(160, 140, 220), rgb(84, 72, 122))),
    os("GNU Guix", &["guix"], logo('\u{f325}', rgb(255, 200, 40), rgb(210, 140, 0))),
    os("Void Linux", &["void"], logo('\u{f32e}', rgb(71, 128, 97), rgb(30, 80, 55))),
    os("Alpine Linux", &["alpine"], logo('\u{f300}', rgb(30, 110, 160), rgb(8, 60, 95))),
    os("Slackware", &["slackware"], logo('\u{f318}', rgb(92, 107, 192), rgb(46, 58, 135))),
    os("Solus", &["solus"], logo('\u{f32d}', rgb(82, 148, 226), rgb(43, 95, 168))),
    os("Qubes OS", &["qubes"], logo('\u{f342}', rgb(56, 116, 216), rgb(31, 78, 154))),
    os("Vanilla OS", &["vanilla"], logo('\u{f366}', rgb(255, 190, 60), rgb(230, 120, 20))),
    os("AOSC OS", &["aosc"], logo('\u{f301}', rgb(70, 130, 200), rgb(30, 70, 140))),
    os("Sabayon", &["sabayon"], logo('\u{f317}', rgb(60, 110, 180), rgb(25, 55, 110))),
    os("Fedora CoreOS", &["coreos"], logo('\u{f305}', rgb(80, 160, 220), rgb(40, 80, 140))),
    os("postmarketOS", &["postmarketos"], logo('\u{f374}', rgb(0, 150, 80), rgb(0, 90, 50))),
    os("Clear Linux", &["clearlinux", "clear linux"], letter('C', rgb(0, 160, 230), rgb(0, 90, 160))),
    os("SteamOS", &["steamos", "holo"], brand('\u{f1b6}', rgb(30, 120, 210), rgb(10, 40, 90))),
    os("ChimeraOS", &["chimeraos"], letter('C', rgb(70, 90, 200), rgb(30, 40, 120))),
    // ---- BSD & others --------------------------------------------------------
    os("FreeBSD", &["freebsd"], logo('\u{f30c}', rgb(235, 0, 40), rgb(150, 0, 20))),
    os("OpenBSD", &["openbsd"], Icon { glyph: Glyph::Logo('\u{f328}'), top: rgb(255, 215, 80), bottom: rgb(220, 160, 0), ink: rgb(30, 30, 30), glow: None }),
    os("NetBSD", &["netbsd"], letter('N', rgb(242, 103, 17), rgb(180, 60, 0))),
    os("GhostBSD", &["ghostbsd"], letter('G', rgb(80, 90, 110), rgb(30, 35, 50))),
    os("illumos", &["illumos", "openindiana", "omnios"], logo('\u{f326}', rgb(240, 120, 40), rgb(180, 60, 10))),
    os("Haiku", &["haiku"], letter('H', rgb(80, 150, 230), rgb(30, 80, 160))),
    os("ReactOS", &["reactos"], letter('R', rgb(80, 120, 220), rgb(30, 60, 150))),
    os("FreeDOS", &["freedos"], letter('F', rgb(60, 140, 90), rgb(25, 80, 50))),
    os("Android", &["android", "bliss", "primeos"], brand('\u{f17b}', rgb(80, 220, 140), rgb(20, 150, 90))),
    os("ChromeOS", &["chromeos", "chromium os", "cloudready", "fydeos"], Icon { glyph: Glyph::Brand('\u{f268}'), top: rgb(255, 255, 255), bottom: rgb(220, 224, 230), ink: rgb(66, 133, 244), glow: None }),
    // ---- Servers & tools -------------------------------------------------------
    os("Proxmox VE", &["proxmox", "pve"], letter('P', rgb(230, 120, 30), rgb(160, 70, 0))),
    os("TrueNAS", &["truenas", "freenas"], letter('T', rgb(0, 150, 220), rgb(0, 80, 140))),
    os("Unraid", &["unraid"], letter('U', rgb(240, 90, 40), rgb(170, 40, 10))),
    os("Ventoy", &["ventoy", "vtoyefi"], letter('V', rgb(60, 180, 120), rgb(20, 110, 70))),
    os("Clonezilla", &["clonezilla"], letter('C', rgb(220, 160, 40), rgb(150, 90, 0))),
    os("GParted Live", &["gparted"], letter('G', rgb(230, 70, 60), rgb(150, 30, 20))),
    os("SystemRescue", &["systemrescue", "sysrcd"], letter('S', rgb(40, 130, 200), rgb(15, 70, 130))),
    os("Memtest86+", &["memtest"], letter('M', rgb(90, 160, 90), rgb(40, 100, 40))),
    os("rEFInd", &["refind"], GEAR),
    os("Linux", &["systemd", "grub", "linux"], LINUX),
];

/// Matches a short name (folder, label, firmware description). The most
/// specific (longest) key wins; the generic "Linux" entry only matches when
/// nothing more specific does, so "Arch Linux" is Arch, not Linux.
pub fn identify(name: &str) -> Option<&'static Os> {
    let n = name.to_lowercase();
    ALL.iter()
        .flat_map(|o| o.keys.iter().map(move |k| (o, *k)))
        .filter(|(_, k)| n.contains(k))
        .max_by_key(|(o, k)| if o.name == "Linux" { 0 } else { k.len() })
        .map(|(o, _)| o)
}

pub fn by_name(name: &str) -> Option<&'static Os> {
    ALL.iter().find(|o| o.name == name)
}

/// Distinctive phrases that identify an OS inside longer text (boot menu
/// titles, `.disk/info`) or inside a loader binary (its signing CA).
/// Unlike `identify`, short or ambiguous keys like "arch" aren't used here.
static CLUES: &[(&str, &str)] = &[
    // Vendor certificates embedded in shim (live USBs boot through it).
    ("canonical ltd. master certificate authority", "Ubuntu"),
    ("debian secure boot ca", "Debian"),
    ("fedora secure boot ca", "Fedora"),
    ("red hat secure boot ca", "Red Hat Enterprise Linux"),
    ("centos secure boot ca", "CentOS"),
    ("rocky enterprise software foundation", "Rocky Linux"),
    ("almalinux os foundation", "AlmaLinux"),
    ("opensuse secure boot ca", "openSUSE"),
    ("suse linux enterprise secure boot ca", "SUSE Linux Enterprise"),
    // Names as they appear in menus and disk info.
    ("ubuntu", "Ubuntu"),
    ("kubuntu", "Kubuntu"),
    ("xubuntu", "Xubuntu"),
    ("lubuntu", "Lubuntu"),
    ("ubuntu mate", "Ubuntu MATE"),
    ("kde neon", "KDE neon"),
    ("linux mint", "Linux Mint"),
    ("linuxmint", "Linux Mint"),
    ("pop!_os", "Pop!_OS"),
    ("pop_os", "Pop!_OS"),
    ("elementary os", "elementary OS"),
    ("zorin", "Zorin OS"),
    ("debian", "Debian"),
    ("devuan", "Devuan"),
    ("mx linux", "MX Linux"),
    ("kali", "Kali Linux"),
    ("parrot", "Parrot OS"),
    ("deepin", "Deepin"),
    ("tails ", "Tails"),
    ("trisquel", "Trisquel"),
    ("fedora", "Fedora"),
    ("nobara", "Nobara"),
    ("bazzite", "Bazzite"),
    ("red hat enterprise", "Red Hat Enterprise Linux"),
    ("centos", "CentOS"),
    ("rocky linux", "Rocky Linux"),
    ("almalinux", "AlmaLinux"),
    ("oracle linux", "Oracle Linux"),
    ("opensuse", "openSUSE"),
    ("tumbleweed", "openSUSE Tumbleweed"),
    ("arch linux", "Arch Linux"),
    ("archiso", "Arch Linux"),
    ("manjaro", "Manjaro"),
    ("endeavouros", "EndeavourOS"),
    ("garuda", "Garuda Linux"),
    ("cachyos", "CachyOS"),
    ("arcolinux", "ArcoLinux"),
    ("artix", "Artix Linux"),
    ("nixos", "NixOS"),
    ("gentoo", "Gentoo"),
    ("guix", "GNU Guix"),
    ("void linux", "Void Linux"),
    ("alpine", "Alpine Linux"),
    ("slackware", "Slackware"),
    ("solus", "Solus"),
    ("qubes", "Qubes OS"),
    ("vanilla os", "Vanilla OS"),
    ("steamos", "SteamOS"),
    ("chimeraos", "ChimeraOS"),
    ("freebsd", "FreeBSD"),
    ("openbsd", "OpenBSD"),
    ("netbsd", "NetBSD"),
    ("haiku", "Haiku"),
    ("reactos", "ReactOS"),
    ("android", "Android"),
    ("chromeos", "ChromeOS"),
    ("chromium os", "ChromeOS"),
    ("proxmox", "Proxmox VE"),
    ("truenas", "TrueNAS"),
    ("unraid", "Unraid"),
    ("clonezilla", "Clonezilla"),
    ("gparted", "GParted Live"),
    ("systemrescue", "SystemRescue"),
    ("memtest86", "Memtest86+"),
    ("ventoy", "Ventoy"),
];

/// The first entries of `CLUES` are signing-CA names found in shim.
const SIGNER_CLUES: usize = 9;

/// Finds the most specific OS named in `text` (ASCII, any case). Among
/// several matches the longest phrase wins, so "Linux Mint" beats "ubuntu"
/// and "Kubuntu" beats "Ubuntu".
pub fn identify_text(text: &[u8]) -> Option<&'static Os> {
    best_clue(text, CLUES)
}

/// Identifies a loader binary by the distro CA certificate embedded in it.
pub fn identify_signer(binary: &[u8]) -> Option<&'static Os> {
    best_clue(binary, &CLUES[..SIGNER_CLUES])
}

fn best_clue(text: &[u8], clues: &[(&str, &'static str)]) -> Option<&'static Os> {
    clues
        .iter()
        .filter(|(needle, _)| contains_ci(text, needle.as_bytes()))
        .max_by_key(|(needle, _)| needle.len())
        .and_then(|(_, name)| by_name(name))
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b))
}
