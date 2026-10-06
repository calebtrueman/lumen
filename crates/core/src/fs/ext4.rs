//! ext2/ext3/ext4 reader (read-only).
//!
//! Handles extent trees and the old direct/indirect block maps, 64-bit
//! group descriptors, meta_bg, inline data and fast symlinks. Hashed (htree)
//! directories need no special code: their index blocks look like empty
//! directory blocks to a linear scan, which is how this reader looks up
//! names. The journal is not replayed (like GRUB): a file system that wasn't
//! unmounted cleanly is read as it is on disk.

use super::*;

const MAGIC: u16 = 0xEF53;

// Incompatible features: refuse anything not listed as understood.
const INCOMPAT_FILETYPE: u32 = 0x2;
const INCOMPAT_RECOVER: u32 = 0x4;
const INCOMPAT_META_BG: u32 = 0x10;
const INCOMPAT_EXTENTS: u32 = 0x40;
const INCOMPAT_64BIT: u32 = 0x80;
const INCOMPAT_MMP: u32 = 0x100;
const INCOMPAT_FLEX_BG: u32 = 0x200;
const INCOMPAT_CSUM_SEED: u32 = 0x2000;
const INCOMPAT_LARGEDIR: u32 = 0x4000;
const INCOMPAT_INLINE_DATA: u32 = 0x8000;
const INCOMPAT_CASEFOLD: u32 = 0x20000;
const INCOMPAT_KNOWN: u32 = INCOMPAT_FILETYPE
    | INCOMPAT_RECOVER
    | INCOMPAT_META_BG
    | INCOMPAT_EXTENTS
    | INCOMPAT_64BIT
    | INCOMPAT_MMP
    | INCOMPAT_FLEX_BG
    | INCOMPAT_CSUM_SEED
    | INCOMPAT_LARGEDIR
    | INCOMPAT_INLINE_DATA
    | INCOMPAT_CASEFOLD;
const RO_COMPAT_SPARSE_SUPER: u32 = 0x1;

const FLAG_EXTENTS: u32 = 0x80000;
const FLAG_INLINE_DATA: u32 = 0x1000_0000;
const FLAG_ENCRYPT: u32 = 0x800;

/// `block` is the 4 KiB starting at byte 1024 (the superblock).
pub fn probe(block: &[u8]) -> bool {
    block.len() >= 0x100 && le16(block, 0x38) == MAGIC
}

pub struct Ext4 {
    vol: Cache,
    block_size: u64,
    inodes_per_group: u32,
    blocks_per_group: u32,
    first_data_block: u64,
    inode_size: u64,
    desc_size: u64,
    group_count: u64,
    incompat: u32,
    ro_compat: u32,
    first_meta_bg: u64,
    uuid: [u8; 16],
    label: String,
}

struct Inode {
    mode: u16,
    size: u64,
    flags: u32,
    block: [u8; 60],
    /// The whole on-disk inode, for in-inode extended attributes.
    raw: Vec<u8>,
}

impl Inode {
    fn kind(&self) -> Kind {
        match self.mode & 0xF000 {
            0x4000 => Kind::Dir,
            0x8000 => Kind::File,
            0xA000 => Kind::Symlink,
            _ => Kind::Other,
        }
    }
}

impl Ext4 {
    pub fn open(mut vol: Cache) -> Result<Self> {
        let mut sb = [0u8; 1024];
        vol.read_at(1024, &mut sb)?;
        if le16(&sb, 0x38) != MAGIC {
            return Err(Error::Corrupt);
        }
        let log = le32(&sb, 0x18);
        if log > 6 {
            return Err(Error::Corrupt);
        }
        let block_size = 1024u64 << log;
        let incompat = le32(&sb, 0x60);
        if incompat & !INCOMPAT_KNOWN != 0 {
            // Compression, journal devices, dirdata, encryption-only
            // layouts...: not something a boot partition uses.
            return Err(Error::Unsupported);
        }
        let rev = le32(&sb, 0x4C);
        let inode_size = if rev >= 1 { le16(&sb, 0x58) as u64 } else { 128 };
        let desc_size = if incompat & INCOMPAT_64BIT != 0 { (le16(&sb, 0xFE) as u64).max(32) } else { 32 };
        let blocks = le32(&sb, 0x4) as u64 | if incompat & INCOMPAT_64BIT != 0 { (le32(&sb, 0x150) as u64) << 32 } else { 0 };
        let blocks_per_group = le32(&sb, 0x20);
        let inodes_per_group = le32(&sb, 0x28);
        let first_data_block = le32(&sb, 0x14) as u64;
        if blocks_per_group == 0 || inodes_per_group == 0 || inode_size < 128 || inode_size > block_size {
            return Err(Error::Corrupt);
        }
        let group_count = (blocks.saturating_sub(first_data_block)).div_ceil(blocks_per_group as u64);
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&sb[0x68..0x78]);
        let label = cstr(&sb[0x78..0x88]);
        Ok(Self {
            vol,
            block_size,
            inodes_per_group,
            blocks_per_group,
            first_data_block,
            inode_size,
            desc_size,
            group_count,
            incompat,
            ro_compat: le32(&sb, 0x64),
            first_meta_bg: le32(&sb, 0x104) as u64,
            uuid,
            label,
        })
    }

    fn read_block(&mut self, n: u64, buf: &mut [u8]) -> Result<()> {
        let off = n.checked_mul(self.block_size).ok_or(Error::Corrupt)?;
        self.vol.read_at(off, buf)
    }

    /// Does this group hold a superblock backup (and so a descriptor copy)?
    fn group_has_super(&self, g: u64) -> bool {
        if self.ro_compat & RO_COMPAT_SPARSE_SUPER == 0 || g <= 1 {
            return true;
        }
        [3u64, 5, 7].iter().any(|&b| {
            let mut p = b;
            while p < g {
                p *= b;
            }
            p == g
        })
    }

    /// Byte offset of group `g`'s descriptor.
    fn desc_offset(&self, g: u64) -> u64 {
        let per_block = self.block_size / self.desc_size;
        let (index, within) = (g / per_block, g % per_block);
        let block = if self.incompat & INCOMPAT_META_BG != 0 && index >= self.first_meta_bg {
            // meta_bg: each descriptor block lives in the first group of
            // the "meta group" it describes, after that group's superblock.
            let first = index * per_block;
            self.first_data_block + first * self.blocks_per_group as u64 + self.group_has_super(first) as u64
        } else {
            self.first_data_block + 1 + index
        };
        block * self.block_size + within * self.desc_size
    }

    fn inode(&mut self, ino: u64) -> Result<Inode> {
        if ino == 0 {
            return Err(Error::Corrupt);
        }
        let g = (ino - 1) / self.inodes_per_group as u64;
        if g >= self.group_count {
            return Err(Error::Corrupt);
        }
        let mut d = [0u8; 64];
        let dlen = self.desc_size.min(64) as usize;
        self.vol.read_at(self.desc_offset(g), &mut d[..dlen])?;
        let table = le32(&d, 0x8) as u64 | if dlen >= 0x2C { (le32(&d, 0x28) as u64) << 32 } else { 0 };
        let index = (ino - 1) % self.inodes_per_group as u64;
        let off = table * self.block_size + index * self.inode_size;
        let mut raw = alloc::vec![0u8; self.inode_size as usize];
        self.vol.read_at(off, &mut raw)?;
        let mut block = [0u8; 60];
        block.copy_from_slice(&raw[0x28..0x64]);
        Ok(Inode {
            mode: le16(&raw, 0),
            size: le32(&raw, 0x4) as u64 | (le32(&raw, 0x6C) as u64) << 32,
            flags: le32(&raw, 0x20),
            block,
            raw,
        })
    }

    /// The physical extents of a file: (logical block, physical block, count).
    /// Unwritten extents are left out, so they read as zeros like holes.
    fn extents(&mut self, inode: &Inode) -> Result<Vec<(u64, u64, u64)>> {
        let mut out = Vec::new();
        if inode.flags & FLAG_EXTENTS != 0 {
            self.walk_extents(&inode.block, 0, &mut out)?;
        } else {
            self.walk_blockmap(inode, &mut out)?;
        }
        Ok(out)
    }

    fn walk_extents(&mut self, node: &[u8], depth_seen: u32, out: &mut Vec<(u64, u64, u64)>) -> Result<()> {
        if node.len() < 12 || le16(node, 0) != 0xF30A || depth_seen > 8 {
            return Err(Error::Corrupt);
        }
        let entries = le16(node, 2) as usize;
        let depth = le16(node, 6);
        if 12 + entries * 12 > node.len() {
            return Err(Error::Corrupt);
        }
        for i in 0..entries {
            let e = &node[12 + i * 12..24 + i * 12];
            if depth == 0 {
                let mut len = le16(e, 4) as u64;
                let start = (le16(e, 6) as u64) << 32 | le32(e, 8) as u64;
                if len > 32768 {
                    continue; // unwritten (preallocated): zeros
                }
                if len == 0 {
                    len = 32768;
                }
                out.push((le32(e, 0) as u64, start, len));
            } else {
                let leaf = le32(e, 4) as u64 | (le16(e, 8) as u64) << 32;
                let mut child = alloc::vec![0u8; self.block_size as usize];
                self.read_block(leaf, &mut child)?;
                self.walk_extents(&child, depth_seen + 1, out)?;
            }
            if out.len() > 1 << 22 {
                return Err(Error::TooBig);
            }
        }
        Ok(())
    }

    /// ext2/ext3 block map: 12 direct pointers, then single, double and
    /// triple indirect blocks.
    fn walk_blockmap(&mut self, inode: &Inode, out: &mut Vec<(u64, u64, u64)>) -> Result<()> {
        let per = self.block_size / 4;
        let nblocks = inode.size.div_ceil(self.block_size);
        let push = |out: &mut Vec<(u64, u64, u64)>, logical: u64, phys: u64| {
            if phys == 0 {
                return;
            }
            match out.last_mut() {
                Some(last) if last.0 + last.2 == logical && last.1 + last.2 == phys => last.2 += 1,
                _ => out.push((logical, phys, 1)),
            }
        };
        for i in 0..12u64.min(nblocks) {
            push(out, i, le32(&inode.block, i as usize * 4) as u64);
        }
        let mut logical = 12;
        for level in 1..=3u32 {
            if logical >= nblocks {
                break;
            }
            let root = le32(&inode.block, (11 + level as usize) * 4) as u64;
            let span = per.pow(level);
            if root != 0 {
                self.walk_indirect(root, level, logical, nblocks, &mut |l, p| push(out, l, p))?;
            }
            logical += span;
        }
        Ok(())
    }

    fn walk_indirect(&mut self, block: u64, level: u32, first: u64, nblocks: u64, f: &mut dyn FnMut(u64, u64)) -> Result<()> {
        let per = self.block_size / 4;
        let mut buf = alloc::vec![0u8; self.block_size as usize];
        self.read_block(block, &mut buf)?;
        let span = per.pow(level - 1);
        for i in 0..per {
            let logical = first + i * span;
            if logical >= nblocks {
                break;
            }
            let p = le32(&buf, i as usize * 4) as u64;
            if p == 0 {
                continue;
            }
            if level == 1 {
                f(logical, p);
            } else {
                self.walk_indirect(p, level - 1, logical, nblocks, f)?;
            }
        }
        Ok(())
    }

    /// The "system.data" in-inode extended attribute: the part of an inline
    /// file or directory that didn't fit in the 60-byte i_block.
    fn inline_xattr(&self, inode: &Inode) -> Vec<u8> {
        let raw = &inode.raw;
        if raw.len() <= 0x84 {
            return Vec::new();
        }
        let start = 0x80 + le16(raw, 0x80) as usize;
        if start + 4 > raw.len() || le32(raw, start) != 0xEA02_0000 {
            return Vec::new();
        }
        let base = start + 4;
        let mut p = base;
        while p + 16 <= raw.len() && le32(raw, p) != 0 {
            let name_len = raw[p] as usize;
            let index = raw[p + 1];
            let value_off = le16(raw, p + 2) as usize;
            let value_size = le32(raw, p + 8) as usize;
            let name = raw.get(p + 16..p + 16 + name_len).unwrap_or(&[]);
            if index == 7 && name == b"data" {
                return raw.get(base + value_off..base + value_off + value_size).map(|v| v.to_vec()).unwrap_or_default();
            }
            p += (16 + name_len + 3) & !3;
        }
        Vec::new()
    }

    fn contents(&mut self, inode: &Inode, limit: u64) -> Result<Vec<u8>> {
        if inode.size > limit {
            return Err(Error::TooBig);
        }
        if inode.flags & FLAG_ENCRYPT != 0 {
            return Err(Error::Unsupported);
        }
        let size = inode.size as usize;
        if inode.flags & FLAG_INLINE_DATA != 0 {
            let mut data = inode.block.to_vec();
            data.extend_from_slice(&self.inline_xattr(inode));
            data.resize(size, 0);
            return Ok(data);
        }
        let mut data = alloc::vec![0u8; size];
        for (logical, phys, count) in self.extents(inode)? {
            let start = logical.saturating_mul(self.block_size);
            if start >= inode.size {
                continue;
            }
            let len = (count * self.block_size).min(inode.size - start) as usize;
            let off = phys.checked_mul(self.block_size).ok_or(Error::Corrupt)?;
            self.vol.read_at(off, &mut data[start as usize..start as usize + len])?;
        }
        Ok(data)
    }

    /// Directory entries as (inode, name, kind).
    fn entries(&mut self, dir: u64) -> Result<Vec<(u64, String, Kind)>> {
        let inode = self.inode(dir)?;
        if inode.kind() != Kind::Dir {
            return Err(Error::NotDir);
        }
        let mut out = Vec::new();
        if inode.flags & FLAG_INLINE_DATA != 0 {
            // Inline directory: parent inode number, then entries, continued
            // in the system.data attribute.
            let parent = le32(&inode.block, 0) as u64;
            out.push((parent, String::from(".."), Kind::Dir));
            self.parse_dirents(&inode.block[4..], &mut out);
            let rest = self.inline_xattr(&inode);
            self.parse_dirents(&rest, &mut out);
            return Ok(out);
        }
        let data = self.contents(&inode, 1 << 30)?;
        for block in data.chunks(self.block_size as usize) {
            self.parse_dirents(block, &mut out);
        }
        Ok(out)
    }

    fn parse_dirents(&self, b: &[u8], out: &mut Vec<(u64, String, Kind)>) {
        let mut p = 0;
        while p + 8 <= b.len() {
            let ino = le32(b, p) as u64;
            let rec_len = le16(b, p + 4) as usize;
            let name_len = b[p + 6] as usize;
            if rec_len < 8 || p + rec_len > b.len() {
                break;
            }
            if ino != 0 && p + 8 + name_len <= b.len() {
                let name = String::from_utf8_lossy(&b[p + 8..p + 8 + name_len]).into_owned();
                let kind = if self.incompat & INCOMPAT_FILETYPE != 0 {
                    match b[p + 7] {
                        1 => Kind::File,
                        2 => Kind::Dir,
                        7 => Kind::Symlink,
                        _ => Kind::Other,
                    }
                } else {
                    Kind::Other // filled in on lookup
                };
                out.push((ino, name, kind));
            }
            p += rec_len;
        }
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

impl FileSystem for Ext4 {
    fn fs_type(&self) -> FsType {
        FsType::Ext4
    }
    fn uuid(&self) -> String {
        uuid_string(&self.uuid)
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn root(&self) -> u64 {
        2
    }
    fn lookup(&mut self, dir: u64, name: &str) -> Result<(u64, Kind)> {
        let (ino, _, kind) = self.entries(dir)?.into_iter().find(|e| e.1 == name).ok_or(Error::NotFound)?;
        let kind = if kind == Kind::Other { self.inode(ino)?.kind() } else { kind };
        Ok((ino, kind))
    }
    fn list(&mut self, dir: u64) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for (ino, name, kind) in self.entries(dir)? {
            if name == "." || name == ".." {
                continue;
            }
            let kind = if kind == Kind::Other { self.inode(ino).map(|i| i.kind()).unwrap_or(Kind::Other) } else { kind };
            out.push(DirEntry { name, kind });
        }
        Ok(out)
    }
    fn file_size(&mut self, node: u64) -> Result<u64> {
        Ok(self.inode(node)?.size)
    }
    fn read(&mut self, node: u64, limit: u64) -> Result<Vec<u8>> {
        let inode = self.inode(node)?;
        match inode.kind() {
            Kind::File => self.contents(&inode, limit),
            Kind::Dir => Err(Error::NotFile),
            _ => Err(Error::NotFile),
        }
    }
    fn read_link(&mut self, node: u64) -> Result<String> {
        let inode = self.inode(node)?;
        if inode.kind() != Kind::Symlink {
            return Err(Error::NotFound);
        }
        let bytes = if inode.size < 60 && inode.flags & (FLAG_EXTENTS | FLAG_INLINE_DATA) == 0 {
            // Fast symlink: the target is stored in i_block itself.
            inode.block[..inode.size as usize].to_vec()
        } else {
            self.contents(&inode, 4096)?
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
