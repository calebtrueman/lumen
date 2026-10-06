//! Read-only file system readers, so Lumen can find and load Linux kernels
//! itself instead of handing over to GRUB. Each reader works on a
//! [`Volume`]: a partition (or whole disk) that the platform can read bytes
//! from. Nothing here ever writes.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub mod btrfs;
pub mod ext4;
pub mod fat;
pub mod path;
pub mod xfs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The platform couldn't read from the disk.
    Io,
    NotFound,
    NotDir,
    NotFile,
    /// The on-disk structures don't make sense.
    Corrupt,
    /// A feature this reader doesn't implement (it won't guess).
    Unsupported,
    /// Too many symlinks, or a file too large to load into memory.
    TooBig,
}

pub type Result<T> = core::result::Result<T, Error>;

/// A partition, readable at any byte offset.
pub trait Volume {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
    /// Size in bytes.
    fn size(&self) -> u64;
}

impl<V: Volume + ?Sized> Volume for &mut V {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size(&self) -> u64 {
        (**self).size()
    }
}

impl<V: Volume + ?Sized> Volume for Box<V> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size(&self) -> u64 {
        (**self).size()
    }
}

/// A volume held in memory (tests, and small images).
impl Volume for Vec<u8> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::Io)?;
        let end = start.checked_add(buf.len()).ok_or(Error::Io)?;
        buf.copy_from_slice(self.get(start..end).ok_or(Error::Io)?);
        Ok(())
    }
    fn size(&self) -> u64 {
        self.len() as u64
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsType {
    Ext4,
    Xfs,
    Btrfs,
    Fat,
}

/// What every reader provides. Paths are absolute, '/'-separated and
/// relative to the file system's root as a boot loader sees it (for btrfs,
/// the top-level subvolume, like GRUB; see [`btrfs`]).
pub trait FileSystem {
    fn fs_type(&self) -> FsType;
    /// The file system UUID, as Linux prints it (`root=UUID=...`).
    fn uuid(&self) -> String;
    fn label(&self) -> String;
    /// Look up one name in a directory. `dir` is an inode/object handle
    /// from [`FileSystem::root`] or an earlier lookup.
    fn lookup(&mut self, dir: u64, name: &str) -> Result<(u64, Kind)>;
    fn root(&self) -> u64;
    fn list(&mut self, dir: u64) -> Result<Vec<DirEntry>>;
    fn file_size(&mut self, node: u64) -> Result<u64>;
    /// Read the whole file. `limit` guards against absurd sizes.
    fn read(&mut self, node: u64, limit: u64) -> Result<Vec<u8>>;
    fn read_link(&mut self, node: u64) -> Result<String>;
}

/// Most files Lumen reads are configs; kernels and initrds are tens of MB.
pub const CONFIG_LIMIT: u64 = 1 << 20;
pub const IMAGE_LIMIT: u64 = 1 << 30;

/// Resolve a path (following symlinks, at most 16) to a node.
pub fn resolve(fs: &mut dyn FileSystem, path: &str) -> Result<(u64, Kind)> {
    resolve_at(fs, "/", path)
}

/// Resolve `path` as if `root` were "/": absolute symlinks and ".." stay
/// inside it, as they would for the installed system once it's running
/// (e.g. inside Ubuntu's "@" btrfs subvolume).
pub fn resolve_at(fs: &mut dyn FileSystem, root: &str, path: &str) -> Result<(u64, Kind)> {
    let mut top = (fs.root(), Kind::Dir);
    for name in path::components(root) {
        top = fs.lookup(top.0, &name)?;
        if top.1 != Kind::Dir {
            return Err(Error::NotDir);
        }
    }
    let mut parts: Vec<String> = path::components(path);
    parts.reverse(); // a stack: next component on top
    let mut stack: Vec<u64> = vec![top.0]; // directory chain, for ".."
    let mut cur = top;
    let mut links = 0;
    while let Some(name) = parts.pop() {
        if cur.1 != Kind::Dir {
            return Err(Error::NotDir);
        }
        match name.as_str() {
            "." => continue,
            ".." => {
                if stack.len() > 1 {
                    stack.pop();
                }
                cur = (*stack.last().unwrap(), Kind::Dir);
                continue;
            }
            _ => {}
        }
        let (node, kind) = fs.lookup(cur.0, &name)?;
        if kind == Kind::Symlink {
            links += 1;
            if links > 16 {
                return Err(Error::TooBig);
            }
            let target = fs.read_link(node)?;
            if target.starts_with('/') {
                stack.truncate(1);
                cur = top;
            }
            for p in path::components(&target).into_iter().rev() {
                parts.push(p);
            }
            continue;
        }
        if kind == Kind::Dir {
            stack.push(node);
        }
        cur = (node, kind);
    }
    Ok(cur)
}

/// Read a file under `root` (see [`resolve_at`]).
pub fn read_file_at(fs: &mut dyn FileSystem, root: &str, path: &str, limit: u64) -> Result<Vec<u8>> {
    match resolve_at(fs, root, path)? {
        (node, Kind::File) => fs.read(node, limit),
        _ => Err(Error::NotFile),
    }
}

pub fn read_file(fs: &mut dyn FileSystem, path: &str, limit: u64) -> Result<Vec<u8>> {
    match resolve(fs, path)? {
        (node, Kind::File) => fs.read(node, limit),
        _ => Err(Error::NotFile),
    }
}

pub fn read_text(fs: &mut dyn FileSystem, path: &str) -> Option<String> {
    let bytes = read_file(fs, path, CONFIG_LIMIT).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn exists(fs: &mut dyn FileSystem, path: &str) -> bool {
    resolve(fs, path).is_ok()
}

pub fn list_dir(fs: &mut dyn FileSystem, path: &str) -> Result<Vec<DirEntry>> {
    match resolve(fs, path)? {
        (node, Kind::Dir) => fs.list(node),
        _ => Err(Error::NotDir),
    }
}

/// Try every reader on a volume.
pub fn open(vol: Box<dyn Volume>) -> Option<Box<dyn FileSystem>> {
    let mut vol = Cache::new(vol);
    let mut probe = [0u8; 4096];
    vol.read_at(0, &mut probe[..]).ok()?;
    let mut sb = [0u8; 4096];
    let have_sb = vol.read_at(1024, &mut sb[..]).is_ok();
    let mut btrfs_sb = [0u8; 4096];
    let have_btrfs = vol.read_at(0x10000, &mut btrfs_sb[..]).is_ok();
    if have_sb && ext4::probe(&sb) {
        return ext4::Ext4::open(vol).ok().map(|f| Box::new(f) as Box<dyn FileSystem>);
    }
    if xfs::probe(&probe) {
        return xfs::Xfs::open(vol).ok().map(|f| Box::new(f) as Box<dyn FileSystem>);
    }
    if have_btrfs && btrfs::probe(&btrfs_sb) {
        return btrfs::Btrfs::open(vol).ok().map(|f| Box::new(f) as Box<dyn FileSystem>);
    }
    if fat::probe(&probe) {
        return fat::Fat::open(vol).ok().map(|f| Box::new(f) as Box<dyn FileSystem>);
    }
    None
}

/// A small cache of recently read 4 KiB blocks in front of a volume.
/// Firmware disk reads are slow per call (BIOS int 13h especially), and
/// metadata walks reread the same blocks constantly.
pub struct Cache {
    vol: Box<dyn Volume>,
    blocks: Vec<(u64, u64, Box<[u8; 4096]>)>, // (block number, last used, data)
    tick: u64,
}

const CACHE_BLOCKS: usize = 256;

impl Cache {
    pub fn new(vol: Box<dyn Volume>) -> Self {
        Self { vol, blocks: Vec::new(), tick: 0 }
    }

    fn block(&mut self, n: u64) -> Result<&[u8; 4096]> {
        self.tick += 1;
        if let Some(i) = self.blocks.iter().position(|b| b.0 == n) {
            self.blocks[i].1 = self.tick;
            return Ok(&self.blocks[i].2);
        }
        let mut data = Box::new([0u8; 4096]);
        let start = n * 4096;
        let len = (self.vol.size().saturating_sub(start)).min(4096) as usize;
        self.vol.read_at(start, &mut data[..len])?;
        let i = if self.blocks.len() < CACHE_BLOCKS {
            self.blocks.push((n, self.tick, data));
            self.blocks.len() - 1
        } else {
            let i = self.blocks.iter().enumerate().min_by_key(|(_, b)| b.1).map(|(i, _)| i).unwrap();
            self.blocks[i] = (n, self.tick, data);
            i
        };
        Ok(&self.blocks[i].2)
    }
}

impl Volume for Cache {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > self.vol.size()) {
            return Err(Error::Io);
        }
        // Large reads (file contents) go straight to the disk.
        if buf.len() >= 64 * 1024 {
            return self.vol.read_at(offset, buf);
        }
        let mut done = 0;
        while done < buf.len() {
            let pos = offset + done as u64;
            let (n, within) = (pos / 4096, (pos % 4096) as usize);
            let take = (4096 - within).min(buf.len() - done);
            let block = self.block(n)?;
            buf[done..done + take].copy_from_slice(&block[within..within + take]);
            done += take;
        }
        Ok(())
    }
    fn size(&self) -> u64 {
        self.vol.size()
    }
}

/// Little-endian field readers for on-disk structures.
pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub(crate) fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}

/// Format 16 bytes as a UUID string, as Linux prints it.
pub fn uuid_string(u: &[u8]) -> String {
    let mut s = String::with_capacity(36);
    for (i, b) in u.iter().take(16).enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        let hex = b"0123456789abcdef";
        s.push(hex[(b >> 4) as usize] as char);
        s.push(hex[(b & 15) as usize] as char);
    }
    s
}
