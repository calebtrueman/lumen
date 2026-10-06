//! Finds installed Linux systems by reading their boot configuration
//! straight from their file systems, so Lumen can start the kernel itself
//! instead of going through GRUB.
//!
//! Sources, in order of preference:
//! - Boot Loader Specification entries (`loader/entries/*.conf`), written by
//!   Fedora, RHEL and derivatives, openSUSE (newer), Bazzite/Silverblue
//!   (ostree), systemd-boot setups;
//! - the first `menuentry` of a GRUB `grub.cfg` (Debian, Ubuntu, Mint,
//!   Arch, openSUSE...), with GRUB's own variables and `search` resolved.
//!
//! Each install is named from its root file system's os-release when that
//! can be read (found from `root=` on the kernel command line), otherwise
//! from the boot entry's title.

use crate::fs::{self, FileSystem, FsType, Kind};
use crate::os::{self, Os};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A file system Lumen can read, with how Linux would refer to it.
pub struct Part {
    pub fs: Box<dyn FileSystem>,
    /// GPT partition GUID or MBR "disksig-NN", lower case; empty if unknown.
    pub partuuid: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Bls,
    Grub,
}

/// One bootable Linux install (its default kernel).
#[derive(Clone)]
pub struct Install {
    pub name: String,
    pub os: Option<&'static Os>,
    /// Kernel version, e.g. "6.8.0-31-generic" (may be empty).
    pub version: String,
    /// Index into the parts list of the file system holding the kernel
    /// and initrds, and their full paths on it.
    pub boot: usize,
    pub kernel: String,
    pub initrds: Vec<String>,
    pub cmdline: String,
    /// The root file system, if it could be found and read.
    pub root: Option<usize>,
    pub source: Source,
}

impl Install {
    /// A stable identity for this install across rescans.
    pub fn id(&self, parts: &[Part]) -> String {
        let root = match self.root {
            Some(r) => parts[r].fs.uuid(),
            None => parts[self.boot].fs.uuid(),
        };
        format!("linux:{root}:{}", self.name)
    }
}

/// Find every Linux install on these file systems.
pub fn discover(parts: &mut [Part]) -> Vec<Install> {
    let mut found: Vec<Install> = Vec::new();
    for i in 0..parts.len() {
        let mut here = Vec::new();
        for top in tops(parts[i].fs.as_mut()) {
            bls_entries(parts, i, &top, &mut here);
            grub_entries(parts, i, &top, &mut here);
        }
        for inst in here {
            // The same install is often reachable twice (a stale grub.cfg
            // next to BLS entries, or two grub.cfg copies): keep the first,
            // which comes from the preferred source.
            let dup = found.iter().any(|f| same_install(parts, f, &inst));
            if !dup {
                found.push(inst);
            }
        }
    }
    found
}

fn same_install(parts: &[Part], a: &Install, b: &Install) -> bool {
    match (a.root, b.root) {
        (Some(x), Some(y)) => x == y && root_flags_subvol(&a.cmdline) == root_flags_subvol(&b.cmdline),
        _ => a.boot == b.boot && a.name == b.name && parts[a.boot].fs.uuid() == parts[b.boot].fs.uuid(),
    }
}

/// Directories that may hold a system's /boot: the file system root, plus
/// top-level btrfs subvolumes (Ubuntu's "@", Fedora's "root").
fn tops(fs: &mut dyn FileSystem) -> Vec<String> {
    let mut out = alloc::vec![String::from("/")];
    if fs.fs_type() == FsType::Btrfs {
        if let Ok(list) = fs::list_dir(fs, "/") {
            for e in list.into_iter().filter(|e| e.kind == Kind::Dir).take(16) {
                if fs::exists(fs, &format!("/{}/boot", e.name)) {
                    out.push(format!("/{}", e.name));
                }
            }
        }
    }
    out
}

fn join(top: &str, rest: &str) -> String {
    fs::path::join(top, rest.trim_start_matches('/'))
}

// ---------------------------------------------------------------------------
// Boot Loader Specification

struct Bls {
    file: String,
    title: String,
    version: String,
    linux: String,
    initrds: Vec<String>,
    options: String,
    sort_key: String,
}

fn bls_entries(parts: &mut [Part], i: usize, top: &str, out: &mut Vec<Install>) {
    for base in ["/", "/boot"] {
        let base = join(top, base);
        let dir = join(&base, "loader/entries");
        let Ok(list) = fs::list_dir(parts[i].fs.as_mut(), &dir) else { continue };
        let grubenv = read_grubenv(parts[i].fs.as_mut(), &base);
        let mut entries: Vec<Bls> = list
            .iter()
            .filter(|e| e.name.ends_with(".conf") && e.kind != Kind::Dir)
            .filter_map(|e| {
                let text = fs::read_text(parts[i].fs.as_mut(), &join(&dir, &e.name))?;
                parse_bls(&e.name, &text, &grubenv)
            })
            .collect();
        // Newest first, as GRUB's blscfg and systemd-boot order them;
        // rescue entries only if there's nothing else.
        entries.sort_by(|a, b| {
            let rescue = |e: &Bls| e.file.contains("rescue") as u8;
            rescue(a)
                .cmp(&rescue(b))
                .then_with(|| a.sort_key.cmp(&b.sort_key))
                .then_with(|| version_cmp(&b.version, &a.version))
                .then_with(|| version_cmp(&b.file, &a.file))
        });
        for e in entries {
            let fsys = parts[i].fs.as_mut();
            // Paths are relative to the partition the entries are on; when
            // that's the root file system, /boot is often spelled out.
            let Some(kernel) = locate(fsys, &[&base, top, "/"], &e.linux) else { continue };
            let initrds: Option<Vec<String>> = e.initrds.iter().map(|p| locate(fsys, &[&base, top, "/"], p)).collect();
            let Some(initrds) = initrds else { continue };
            let version = if e.version.is_empty() { version_from_path(&e.linux) } else { e.version.clone() };
            out.push(finish(parts, i, e.title, version, kernel, initrds, e.options, Source::Bls));
            break; // the default entry is enough
        }
    }
}

fn parse_bls(file: &str, text: &str, grubenv: &[(String, String)]) -> Option<Bls> {
    let mut e = Bls {
        file: file.to_string(),
        title: String::new(),
        version: String::new(),
        linux: String::new(),
        initrds: Vec::new(),
        options: String::new(),
        sort_key: String::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace()) {
            Some((k, v)) => (k, v.trim()),
            None => continue,
        };
        match key {
            "title" => e.title = value.to_string(),
            "version" => e.version = value.to_string(),
            "linux" => e.linux = value.to_string(),
            "initrd" => e.initrds.extend(value.split_whitespace().map(|s| s.to_string())),
            "options" => {
                if !e.options.is_empty() {
                    e.options.push(' ');
                }
                e.options.push_str(value);
            }
            "sort-key" => e.sort_key = value.to_string(),
            _ => {}
        }
    }
    if e.linux.is_empty() {
        return None; // e.g. an efi= entry for memtest
    }
    // Older Fedora/RHEL entries say "options $kernelopts", defined in grubenv.
    e.options = expand(&e.options, grubenv);
    e.initrds = e.initrds.iter().map(|p| expand(p, grubenv)).filter(|p| !p.is_empty()).collect();
    Some(e)
}

fn read_grubenv(fs: &mut dyn FileSystem, base: &str) -> Vec<(String, String)> {
    for p in ["grub2/grubenv", "grub/grubenv", "efi/EFI/fedora/grubenv"] {
        if let Some(text) = fs::read_text(fs, &join(base, p)) {
            return text
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// GRUB

fn grub_entries(parts: &mut [Part], i: usize, top: &str, out: &mut Vec<Install>) {
    for p in ["/boot/grub/grub.cfg", "/grub/grub.cfg", "/boot/grub2/grub.cfg", "/grub2/grub.cfg"] {
        let path = join(top, p);
        let Some(text) = fs::read_text(parts[i].fs.as_mut(), &path) else { continue };
        let grubenv = read_grubenv(parts[i].fs.as_mut(), fs::path::parent(fs::path::parent(&path)));
        let Some(entry) = parse_grub(&text, &grubenv) else { continue };
        // Which file system the paths are on: the one `search` picked, or
        // else the one grub.cfg itself is on.
        let target = match &entry.search {
            Some(Search::Uuid(u)) => (0..parts.len()).find(|&j| parts[j].fs.uuid().eq_ignore_ascii_case(u)),
            Some(Search::Label(l)) => (0..parts.len()).find(|&j| parts[j].fs.label() == *l),
            Some(Search::File(f)) => (0..parts.len()).find(|&j| fs::exists(parts[j].fs.as_mut(), f)),
            None => Some(i),
        };
        let Some(t) = target else { continue };
        let fsys = parts[t].fs.as_mut();
        let Some(kernel) = locate(fsys, &["/", top], &entry.linux) else { continue };
        let initrds: Option<Vec<String>> = entry.initrds.iter().map(|p| locate(fsys, &["/", top], p)).collect();
        let Some(initrds) = initrds else { continue };
        let version = version_from_path(&entry.linux);
        out.push(finish(parts, t, entry.title, version, kernel, initrds, entry.args, Source::Grub));
        return;
    }
}

enum Search {
    Uuid(String),
    Label(String),
    File(String),
}

struct GrubEntry {
    title: String,
    linux: String,
    args: String,
    initrds: Vec<String>,
    search: Option<Search>,
}

/// Interpret just enough of grub.cfg to find the first menu entry that
/// boots Linux: variables set at the top level, `if` blocks (taking the
/// first branch, the usual case), and skipping `submenu` and `function`
/// blocks. Everything else is ignored.
fn parse_grub(text: &str, grubenv: &[(String, String)]) -> Option<GrubEntry> {
    let mut vars: Vec<(String, String)> = grubenv.to_vec();
    let lines = logical_lines(text);
    let mut skip_depth = 0usize; // inside a submenu/function body
    let mut if_stack: Vec<bool> = Vec::new(); // is this branch taken?
    let mut entry: Option<GrubEntry> = None;
    for words in lines.iter().map(|l| split_words(l)) {
        let Some(cmd) = words.first().map(|s| s.as_str()) else { continue };
        let active = if_stack.iter().all(|&t| t);
        if skip_depth > 0 {
            if cmd == "}" {
                skip_depth -= 1;
            } else if words.last().map(|s| s.as_str()) == Some("{") {
                skip_depth += 1;
            }
            continue;
        }
        match cmd {
            "if" => {
                if_stack.push(true);
                continue;
            }
            "elif" | "else" => {
                if let Some(t) = if_stack.last_mut() {
                    *t = false;
                }
                continue;
            }
            "fi" => {
                if_stack.pop();
                continue;
            }
            "then" => continue,
            _ => {}
        }
        if !active {
            continue;
        }
        if let Some(e) = entry.as_mut() {
            match cmd {
                "}" => {
                    let done = entry.take().unwrap();
                    if !done.linux.is_empty() {
                        return Some(done);
                    }
                }
                "linux" | "linuxefi" | "linux16" if words.len() > 1 => {
                    let w: Vec<String> = words[1..].iter().map(|w| expand(w, &vars)).filter(|w| !w.is_empty()).collect();
                    if let Some((k, rest)) = w.split_first() {
                        e.linux = strip_device(k);
                        e.args = rest.join(" ");
                    }
                }
                "initrd" | "initrdefi" | "initrd16" => {
                    e.initrds = words[1..].iter().map(|w| strip_device(&expand(w, &vars))).filter(|w| !w.is_empty()).collect();
                }
                "search" | "search.fs_uuid" | "search.fs_label" | "search.file" => {
                    if let Some(s) = parse_search(&words, &vars) {
                        e.search = Some(s);
                    }
                }
                "set" => set_var(&mut vars, &words),
                _ => {}
            }
            continue;
        }
        match cmd {
            "menuentry" => {
                let title = words.get(1).map(|t| expand(t, &vars)).unwrap_or_default();
                entry = Some(GrubEntry { title, linux: String::new(), args: String::new(), initrds: Vec::new(), search: None });
            }
            "submenu" | "function" => {
                if words.last().map(|s| s.as_str()) == Some("{") {
                    skip_depth = 1;
                }
            }
            "set" => set_var(&mut vars, &words),
            _ if cmd.contains('=') && !cmd.starts_with('[') => set_var(&mut vars, &["set".to_string(), cmd.to_string()]),
            _ => {}
        }
    }
    None
}

fn set_var(vars: &mut Vec<(String, String)>, words: &[String]) {
    let Some(assign) = words.get(1) else { return };
    let Some((k, v)) = assign.split_once('=') else { return };
    let v = expand(v, vars);
    vars.retain(|(name, _)| name != k);
    vars.push((k.to_string(), v));
}

fn parse_search(words: &[String], vars: &[(String, String)]) -> Option<Search> {
    let mut kind = match words[0].as_str() {
        "search.fs_uuid" => 'u',
        "search.fs_label" => 'l',
        "search.file" => 'f',
        _ => 'f',
    };
    let mut value = None;
    let mut it = words[1..].iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "--fs-uuid" | "-u" => kind = 'u',
            "--label" | "-l" => kind = 'l',
            "--file" | "-f" => kind = 'f',
            "--set" => {
                // "--set root" (separate argument) or "--set=root".
                it.next();
            }
            s if s.starts_with("--") => {}
            s if s.starts_with('-') && s.len() == 2 => {}
            s => {
                if value.is_none() {
                    value = Some(expand(s, vars));
                }
            }
        }
    }
    let v = value?;
    Some(match kind {
        'u' => Search::Uuid(v),
        'l' => Search::Label(v),
        _ => Search::File(v),
    })
}

/// "($root)/vmlinuz" or "(hd0,gpt2)/vmlinuz" -> "/vmlinuz".
fn strip_device(p: &str) -> String {
    if p.starts_with('(') {
        if let Some(i) = p.find(')') {
            return p[i + 1..].to_string();
        }
    }
    p.to_string()
}

/// Join backslash-continued lines and split on newlines and ';'
/// (outside quotes), dropping comments.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if !sq => {
                match chars.next() {
                    Some('\n') => {}
                    Some(n) => {
                        cur.push('\\');
                        cur.push(n);
                    }
                    None => {}
                }
                continue;
            }
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            '#' if !sq && !dq && (cur.is_empty() || cur.ends_with(char::is_whitespace)) => {
                while chars.peek().is_some_and(|&c| c != '\n') {
                    chars.next();
                }
                continue;
            }
            '\n' | ';' if !sq && !dq => {
                if !cur.trim().is_empty() {
                    out.push(core::mem::take(&mut cur));
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
        // "menuentry 'x' {" opens a block; "}" may share a line with
        // other commands, so split it off.
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    // Put a lone "}" or "{ ... }" on its own line.
    let mut split = Vec::new();
    for l in out {
        let t = l.trim();
        if t.len() > 1 && t.ends_with('}') && !t.ends_with("\\}") && !t.contains('{') {
            split.push(t[..t.len() - 1].to_string());
            split.push(String::from("}"));
        } else if t.starts_with('}') && t.len() > 1 {
            split.push(String::from("}"));
            split.push(t[1..].to_string());
        } else {
            split.push(l);
        }
    }
    split
}

/// Split a command into words, removing quotes but keeping `$var`
/// references for later expansion (single-quoted text is protected by
/// escaping its '$' signs).
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let (mut sq, mut dq) = (false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !dq => {
                sq = !sq;
                have = true;
            }
            '"' if !sq => {
                dq = !dq;
                have = true;
            }
            '\\' if !sq => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '$' if sq => cur.push('\u{1}'),
            c if c.is_whitespace() && !sq && !dq => {
                if have || !cur.is_empty() {
                    words.push(core::mem::take(&mut cur));
                    have = false;
                }
            }
            c => cur.push(c),
        }
    }
    if have || !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Replace `$name` and `${name}` with their values (unknown -> empty).
fn expand(s: &str, vars: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1}' {
            out.push('$');
            continue;
        }
        if c != '$' {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        if chars.peek() == Some(&'{') {
            chars.next();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        if let Some((_, v)) = vars.iter().rev().find(|(k, _)| *k == name) {
            out.push_str(v);
        }
    }
    // Expanding an empty variable can leave doubled spaces.
    let mut tidy = String::new();
    for w in out.split(' ').filter(|w| !w.is_empty()) {
        if !tidy.is_empty() {
            tidy.push(' ');
        }
        tidy.push_str(w);
    }
    if out.starts_with(' ') || out.ends_with(' ') || out.contains("  ") { tidy } else { out }
}

// ---------------------------------------------------------------------------
// Shared

/// Find a boot file: try the path under each base directory in turn.
fn locate(fs: &mut dyn FileSystem, bases: &[&str], path: &str) -> Option<String> {
    for base in bases {
        let full = join(base, path);
        if let Ok((_, Kind::File)) = fs::resolve(fs, &full) {
            return Some(full);
        }
    }
    None
}

fn finish(
    parts: &mut [Part],
    boot: usize,
    title: String,
    version: String,
    kernel: String,
    initrds: Vec<String>,
    cmdline: String,
    source: Source,
) -> Install {
    let root = find_root(parts, &cmdline);
    let release = root.and_then(|r| os_release(parts[r].fs.as_mut(), &cmdline));
    let (name, os) = naming(release.as_ref(), &title);
    // GRUB passes the kernel's own path first; some initramfs scripts
    // and tools read it.
    let cmdline = if cmdline.contains("BOOT_IMAGE=") || source == Source::Bls {
        cmdline
    } else {
        format!("BOOT_IMAGE={kernel} {cmdline}").trim_end().to_string()
    };
    Install { name, os, version, boot, kernel, initrds, cmdline, root, source }
}

fn arg<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline.split_whitespace().filter_map(|w| w.strip_prefix(key)).last()
}

fn root_flags_subvol(cmdline: &str) -> String {
    arg(cmdline, "rootflags=")
        .and_then(|f| f.split(',').find_map(|o| o.strip_prefix("subvol=")))
        .map(|s| format!("/{}", s.trim_start_matches('/')))
        .unwrap_or_default()
}

fn find_root(parts: &mut [Part], cmdline: &str) -> Option<usize> {
    let root = arg(cmdline, "root=")?;
    let root = root.strip_prefix("/dev/disk/by-uuid/").map(|u| format!("UUID={u}")).unwrap_or_else(|| root.to_string());
    let root = root.strip_prefix("/dev/disk/by-partuuid/").map(|u| format!("PARTUUID={u}")).unwrap_or(root);
    let root = root.strip_prefix("/dev/disk/by-label/").map(|u| format!("LABEL={u}")).unwrap_or(root);
    if let Some(u) = root.strip_prefix("UUID=") {
        return parts.iter().position(|p| p.fs.uuid().eq_ignore_ascii_case(u));
    }
    if let Some(u) = root.strip_prefix("PARTUUID=") {
        return parts.iter().position(|p| !p.partuuid.is_empty() && p.partuuid.eq_ignore_ascii_case(u));
    }
    if let Some(l) = root.strip_prefix("LABEL=") {
        return parts.iter().position(|p| p.fs.label() == l);
    }
    None // /dev/sdXN, LVM, LUKS...: can't tell from here
}

struct Release {
    name: String,
    pretty: String,
    id: String,
    id_like: String,
}

fn os_release(fs: &mut dyn FileSystem, cmdline: &str) -> Option<Release> {
    let top = root_flags_subvol(cmdline);
    let top = if top.is_empty() { String::from("/") } else { top };
    // ostree systems (Silverblue, Bazzite...) boot a deployment inside the
    // root file system; its path is on the command line.
    let mut dirs = Vec::new();
    if let Some(o) = arg(cmdline, "ostree=") {
        dirs.push(o.to_string());
    }
    dirs.push(String::from("/"));
    for d in dirs {
        for p in ["etc/os-release", "usr/lib/os-release"] {
            let Ok(bytes) = fs::read_file_at(fs, &top, &join(&d, p), fs::CONFIG_LIMIT) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let get = |k: &str| {
                text.lines()
                    .filter_map(|l| l.strip_prefix(k)?.strip_prefix('='))
                    .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
                    .next()
                    .unwrap_or_default()
            };
            return Some(Release { name: get("NAME"), pretty: get("PRETTY_NAME"), id: get("ID"), id_like: get("ID_LIKE") });
        }
    }
    None
}

/// The card title and icon: what os-release says, matched against Lumen's
/// own OS list so names and icons agree with the rest of the menu.
fn naming(release: Option<&Release>, title: &str) -> (String, Option<&'static Os>) {
    if let Some(r) = release {
        let known = os::identify(&r.id)
            .filter(|o| o.name != "Linux")
            .or_else(|| os::identify_text(r.name.as_bytes()))
            .or_else(|| os::identify_text(r.pretty.as_bytes()));
        let name = match known {
            Some(o) => o.name.to_string(),
            None if !r.name.is_empty() => r.name.clone(),
            None => r.pretty.clone(),
        };
        let icon = known.or_else(|| r.id_like.split_whitespace().find_map(|l| os::identify(l).filter(|o| o.name != "Linux")));
        if !name.is_empty() {
            return (name, known.or(icon));
        }
    }
    let clean = clean_title(title);
    match os::identify_text(clean.as_bytes()) {
        Some(o) => (o.name.to_string(), Some(o)),
        None if !clean.is_empty() => (clean, None),
        None => (String::from("Linux"), None),
    }
}

/// "Fedora Linux (6.8.9-300.fc40.x86_64) 40 (Workstation Edition)" ->
/// "Fedora Linux 40 (Workstation Edition)"; drops "(ostree:0)" too.
fn clean_title(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(start) = rest.find('(') {
        let Some(len) = rest[start..].find(')') else { break };
        let inner = &rest[start + 1..start + len];
        out.push_str(&rest[..start]);
        let kernelish = inner.starts_with(|c: char| c.is_ascii_digit()) || inner.starts_with("ostree");
        if !kernelish {
            out.push_str(&rest[start..=start + len]);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    let words: Vec<&str> = out.split_whitespace().collect();
    words.join(" ")
}

/// "/boot/vmlinuz-6.8.0-31-generic" -> "6.8.0-31-generic".
fn version_from_path(p: &str) -> String {
    let name = p.rsplit('/').next().unwrap_or(p);
    for prefix in ["vmlinuz-", "vmlinux-", "linux-", "Image-", "bzImage-", "kernel-"] {
        // Arch names kernels after their package ("vmlinuz-linux",
        // "vmlinuz-linux-lts"): that's not a version.
        if let Some(v) = name.strip_prefix(prefix).filter(|v| v.starts_with(|c: char| c.is_ascii_digit())) {
            return v.to_string();
        }
    }
    String::new()
}

/// Compare version strings the way rpm/dpkg roughly do: runs of digits
/// numerically, everything else as text.
pub fn version_cmp(a: &str, b: &str) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let nb = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (da, db) = (trim_zeros(&a[..na]), trim_zeros(&b[..nb]));
                let o = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
                if o != Ordering::Equal {
                    return o;
                }
                a = &a[na..];
                b = &b[nb..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let z = d.iter().take_while(|&&c| c == b'0').count();
    &d[z.min(d.len().saturating_sub(1))..]
}

/// The kernel's version as stored in its x86 boot header (empty if the
/// image isn't an x86 bzImage), e.g. "6.8.0-31-generic (buildd@...) #31".
pub fn bzimage_version(image: &[u8]) -> String {
    if image.len() < 0x300 || &image[0x202..0x206] != b"HdrS" {
        return String::new();
    }
    let ptr = u16::from_le_bytes([image[0x20E], image[0x20F]]) as usize;
    let at = ptr + 0x200;
    let Some(bytes) = image.get(at..(at + 128).min(image.len())) else { return String::new() };
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Can this kernel take its initrd from Lumen over the EFI LoadFile2
/// protocol (Linux 5.8+ on x86)? Unknown versions are assumed to.
pub fn supports_initrd_loadfile2(image: &[u8]) -> bool {
    let v = bzimage_version(image);
    let mut nums = v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().unwrap_or(0));
    match (nums.next(), nums.next()) {
        (Some(major), Some(minor)) => (major, minor) >= (5, 8),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubuntu_grub_cfg() {
        let cfg = r#"
if [ -s $prefix/grubenv ]; then
  set have_grubenv=true
  load_env
fi
set default="0"
function load_video {
  if [ x$feature_all_video_module = xy ]; then
    insmod all_video
  fi
}
if [ "${recordfail}" != 1 ]; then
  if [ -e ${prefix}/gfxblacklist.txt ]; then
    set linux_gfx_mode=keep
  else
    set linux_gfx_mode=keep
  fi
else
  set linux_gfx_mode=text
fi
export linux_gfx_mode
if [ "$linux_gfx_mode" != "text" ]; then load_video; fi
if [ "${linux_gfx_mode}" != "text" ]; then
  set vt_handoff=vt.handoff=7
else
  set vt_handoff=
fi
menuentry 'Ubuntu' --class ubuntu --class gnu-linux --class gnu --class os $menuentry_id_option 'gnulinux-simple-0b9f' {
	recordfail
	load_video
	gfxmode $linux_gfx_mode
	insmod gzio
	if [ x$grub_platform = xxen ]; then insmod xzio; insmod lzopio; fi
	insmod part_gpt
	insmod ext2
	search --no-floppy --fs-uuid --set=root 0b9f-uuid
	linux	/boot/vmlinuz-6.8.0-31-generic root=UUID=0b9f-uuid ro  quiet splash $vt_handoff
	initrd	/boot/initrd.img-6.8.0-31-generic
}
submenu 'Advanced options for Ubuntu' $menuentry_id_option 'gnulinux-advanced-0b9f' {
	menuentry 'Ubuntu, with Linux 6.8.0-31-generic' {
		linux	/boot/vmlinuz-6.8.0-31-generic root=UUID=0b9f-uuid ro
	}
}
"#;
        let e = parse_grub(cfg, &[]).unwrap();
        assert_eq!(e.title, "Ubuntu");
        assert_eq!(e.linux, "/boot/vmlinuz-6.8.0-31-generic");
        assert_eq!(e.args, "root=UUID=0b9f-uuid ro quiet splash vt.handoff=7");
        assert_eq!(e.initrds, ["/boot/initrd.img-6.8.0-31-generic"]);
        assert!(matches!(e.search, Some(Search::Uuid(ref u)) if u == "0b9f-uuid"));
    }

    #[test]
    fn opensuse_grub_cfg_with_hints_and_firmware_entry() {
        let cfg = r#"
menuentry 'UEFI Firmware Settings' $menuentry_id_option 'uefi-firmware' {
	fwsetup
}
menuentry 'openSUSE Tumbleweed'  --class opensuse --class gnu-linux --class gnu --class os $menuentry_id_option 'gnulinux-simple-abcd' {
	load_video
	set gfxpayload=keep
	insmod gzio
	if [ x$feature_platform_search_hint = xy ]; then
	  search --no-floppy --fs-uuid --set=root --hint-bios=hd0,gpt2 abcd
	else
	  search --no-floppy --fs-uuid --set=root abcd
	fi
	echo	'Loading Linux 6.9.1-1-default ...'
	linux	/boot/vmlinuz-6.9.1-1-default root=UUID=abcd  ${extra_cmdline} splash=silent quiet security=apparmor mitigations=auto
	echo	'Loading initial ramdisk ...'
	initrd	/boot/initrd-6.9.1-1-default
}
"#;
        let e = parse_grub(cfg, &[]).unwrap();
        assert_eq!(e.title, "openSUSE Tumbleweed");
        assert_eq!(e.args, "root=UUID=abcd splash=silent quiet security=apparmor mitigations=auto");
        assert_eq!(e.initrds, ["/boot/initrd-6.9.1-1-default"]);
    }

    #[test]
    fn arch_grub_cfg_with_microcode() {
        let cfg = "menuentry 'Arch Linux' --class arch {\n search --no-floppy --fs-uuid --set=root BOOT\n echo 'Loading Linux linux ...'\n linux /vmlinuz-linux root=UUID=ROOT rw loglevel=3 quiet\n initrd /intel-ucode.img /initramfs-linux.img\n}\n";
        let e = parse_grub(cfg, &[]).unwrap();
        assert_eq!(e.linux, "/vmlinuz-linux");
        assert_eq!(e.initrds, ["/intel-ucode.img", "/initramfs-linux.img"]);
    }

    #[test]
    fn fedora_bls_with_kernelopts() {
        let env = [(String::from("kernelopts"), String::from("root=UUID=r ro rhgb quiet"))];
        let e = parse_bls(
            "abc-6.8.9-300.fc40.x86_64.conf",
            "title Fedora Linux (6.8.9-300.fc40.x86_64) 40 (Workstation Edition)\nversion 6.8.9-300.fc40.x86_64\nlinux /vmlinuz-6.8.9-300.fc40.x86_64\ninitrd /initramfs-6.8.9-300.fc40.x86_64.img $tuned_initrd\noptions $kernelopts $tuned_params\ngrub_users $grub_users\ngrub_arg --unrestricted\ngrub_class fedora\n",
            &env,
        )
        .unwrap();
        assert_eq!(e.options, "root=UUID=r ro rhgb quiet");
        assert_eq!(e.initrds, ["/initramfs-6.8.9-300.fc40.x86_64.img"]);
        assert_eq!(clean_title(&e.title), "Fedora Linux 40 (Workstation Edition)");
    }

    #[test]
    fn kernel_file_versions() {
        assert_eq!(version_from_path("/boot/vmlinuz-6.8.0-31-generic"), "6.8.0-31-generic");
        assert_eq!(version_from_path("/boot/vmlinuz-linux"), "");
        assert_eq!(version_from_path("/vmlinuz-linux-lts"), "");
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("Bazzite 40 (FROM Fedora Kinoite) (ostree:0)"), "Bazzite 40 (FROM Fedora Kinoite)");
    }

    #[test]
    fn versions() {
        use core::cmp::Ordering::*;
        assert_eq!(version_cmp("6.10.1", "6.9.12"), Greater);
        assert_eq!(version_cmp("6.8.0-31", "6.8.0-100"), Less);
        assert_eq!(version_cmp("5.10", "5.10"), Equal);
    }

    #[test]
    fn quoting() {
        assert_eq!(split_words("menuentry 'A $b' \"c d\" e\\ f"), ["menuentry", "A \u{1}b", "c d", "e f"]);
        assert_eq!(expand("A \u{1}b", &[]), "A $b");
    }
}
