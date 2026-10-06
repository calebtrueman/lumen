//! Checks each file system reader against real images made by the real
//! mkfs tools (tools/test/make-fs-fixtures.sh): every file's SHA-256 and
//! every symlink must match the manifest. Skips if the fixtures aren't built.
//!
//!   cargo test -p lumen-core --target aarch64-apple-darwin --test fs

use lumen_core::fs::{self, FileSystem, FsType, Kind, Volume};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

struct FileVolume(std::fs::File, u64);

impl Volume for FileVolume {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> fs::Result<()> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_| fs::Error::Io)?;
        self.0.read_exact(buf).map_err(|_| fs::Error::Io)
    }
    fn size(&self) -> u64 {
        self.1
    }
}

fn fixtures() -> Option<PathBuf> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures");
    d.join("manifest.txt").exists().then_some(d)
}

fn open(name: &str) -> Option<Box<dyn FileSystem>> {
    let dir = fixtures()?;
    let f = std::fs::File::open(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}; rerun tools/test/make-fs-fixtures.sh"));
    let len = f.metadata().unwrap().len();
    Some(fs::open(Box::new(FileVolume(f, len))).unwrap_or_else(|| panic!("{name}: not recognised")))
}

fn uuid_of(img: &str) -> String {
    let ids = std::fs::read_to_string(fixtures().unwrap().join("ids.txt")).unwrap();
    let line = ids.lines().find(|l| l.ends_with(&format!(" {img}"))).unwrap();
    line.split_whitespace().find_map(|w| w.strip_prefix("UUID=")).unwrap().to_lowercase()
}

/// Check every manifest entry under `prefix` (e.g. "/@" for a subvolume).
fn check(img: &str, prefix: &str, ty: FsType, skip_links: bool) {
    let Some(mut fsys) = open(img) else {
        eprintln!("fixtures missing; run tools/test/make-fs-fixtures.sh");
        return;
    };
    let f = fsys.as_mut();
    assert_eq!(f.fs_type(), ty);
    if ty != FsType::Fat {
        assert_eq!(f.uuid(), uuid_of(img), "{img}: uuid");
    }
    let manifest = std::fs::read_to_string(fixtures().unwrap().join("manifest.txt")).unwrap();
    let mut n = 0;
    for line in manifest.lines() {
        let mut parts = line.splitn(3, ' ');
        let (a, b) = (parts.next().unwrap(), parts.next().unwrap());
        if a == "link" {
            let path = format!("{prefix}{}", parts.next().unwrap());
            if skip_links {
                continue;
            }
            let (node, kind) = lookup_nofollow(f, &path);
            assert_eq!(kind, Kind::Symlink, "{img}{path}");
            assert_eq!(f.read_link(node).unwrap(), b, "{img}{path}");
            // And following it (inside the subvolume) reaches a file.
            fs::read_file_at(f, prefix, &path[prefix.len()..], fs::IMAGE_LIMIT).unwrap_or_else(|e| panic!("{img}{path}: {e:?}"));
        } else {
            let path = format!("{prefix}{}", line.split_once(' ').unwrap().1);
            let data = fs::read_file(f, &path, fs::IMAGE_LIMIT).unwrap_or_else(|e| panic!("{img}{path}: {e:?}"));
            assert_eq!(format!("{:x}", Sha256::digest(&data)), a, "{img}{path}: contents ({} bytes)", data.len());
            n += 1;
        }
    }
    assert!(n > 3000);
    // Directory listing.
    let big = fs::list_dir(f, &format!("{prefix}/big")).unwrap();
    assert_eq!(big.iter().filter(|e| e.name.starts_with("file-")).count(), 3000, "{img}: /big listing");
    let boot = fs::list_dir(f, &format!("{prefix}/boot")).unwrap();
    assert!(boot.iter().any(|e| e.name == "grub" && e.kind == Kind::Dir), "{img}: /boot listing {boot:?}");
    assert!(matches!(fs::read_file(f, &format!("{prefix}/nope"), 10), Err(fs::Error::NotFound)));
}

fn lookup_nofollow(f: &mut dyn FileSystem, path: &str) -> (u64, Kind) {
    let parent = fs::path::parent(path);
    let name = path.rsplit('/').next().unwrap();
    let (dir, _) = fs::resolve(f, parent).unwrap();
    f.lookup(dir, name).unwrap()
}

#[test]
fn ext4() {
    check("ext4.img", "", FsType::Ext4, false);
}

#[test]
fn ext2() {
    check("ext2.img", "", FsType::Ext4, false);
}

#[test]
fn ext4_inline_data() {
    check("ext4-inline.img", "", FsType::Ext4, false);
}

#[test]
fn xfs() {
    check("xfs.img", "", FsType::Xfs, false);
}

#[test]
fn btrfs_subvolumes_zstd() {
    check("btrfs.img", "/@", FsType::Btrfs, false);
    check("btrfs.img", "/@zlib", FsType::Btrfs, false);
}

#[test]
fn btrfs_zlib() {
    check("btrfs-zlib.img", "", FsType::Btrfs, false);
}

#[test]
fn btrfs_lzo() {
    check("btrfs-lzo.img", "", FsType::Btrfs, false);
}

#[test]
fn btrfs_plain() {
    check("btrfs-plain.img", "", FsType::Btrfs, false);
}

#[test]
fn fat32() {
    check("fat32.img", "", FsType::Fat, true);
}

#[test]
fn fat16() {
    check("fat16.img", "", FsType::Fat, true);
}

