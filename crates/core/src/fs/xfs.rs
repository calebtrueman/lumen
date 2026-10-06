//! XFS reader (read-only): v4 and v5 (CRC) file systems.
//!
//! Everything on disk is big-endian. Inode numbers encode their location:
//! `| AG number | block within AG (agblklog bits) | slot in block (inopblog bits) |`,
//! and file system block numbers in extents likewise are `| AG | block |`,
//! so neither is a plain linear address.

use super::*;
use alloc::collections::BTreeSet;

pub fn probe(block: &[u8]) -> bool {
    block.len() >= 4 && &block[..4] == b"XFSB"
}

/// v5 incompatible features this reader understands: ftype, sparse inode
/// chunks, meta UUID, bigtime, needs-repair, 64-bit extent counts, exchange
/// range, parent pointers, metadata directory. None changes how regular
/// files and directories are found and read.
const KNOWN_INCOMPAT: u32 = 0x1ff;
const XFS_DIR2_LEAF_OFFSET: u64 = 1 << 35; // directory data blocks live below 32 GiB
const MAX_EXTENTS: usize = 1 << 22;

pub struct Xfs {
    vol: Cache,
    block_size: u64,
    ag_blocks: u64,
    ag_count: u64,
    agblklog: u32,
    inopblog: u32,
    inode_size: u64,
    dirblklog: u32,
    root: u64,
    v5: bool,
    ftype: bool,
    uuid: [u8; 16],
    label: String,
}

struct Inode {
    mode: u16,
    format: u8,
    size: u64,
    nextents: u64,
    flags: u16,
    /// The data fork, straight from the inode.
    fork: Vec<u8>,
}

impl Inode {
    fn kind(&self) -> Kind {
        mode_kind(self.mode)
    }
}

fn mode_kind(mode: u16) -> Kind {
    match mode & 0xf000 {
        0x8000 => Kind::File,
        0x4000 => Kind::Dir,
        0xa000 => Kind::Symlink,
        _ => Kind::Other,
    }
}

/// A data fork mapping: `len` blocks of the file from block `off` are at
/// file system block `block`. Unwritten (preallocated) extents read as zeros.
#[derive(Clone, Copy)]
struct Extent {
    off: u64,
    block: u64,
    len: u64,
    unwritten: bool,
}

/// A packed 128-bit extent record: 1 bit unwritten flag, 54 bits file
/// offset, 52 bits start block, 21 bits length.
fn extent_rec(b: &[u8], o: usize) -> Extent {
    let (l0, l1) = (be64(b, o), be64(b, o + 8));
    Extent {
        unwritten: l0 >> 63 != 0,
        off: (l0 & ((1 << 63) - 1)) >> 9,
        block: ((l0 & 0x1ff) << 43) | (l1 >> 21),
        len: l1 & ((1 << 21) - 1),
    }
}

impl Xfs {
    pub fn open(mut vol: Cache) -> Result<Self> {
        let mut sb = [0u8; 512];
        vol.read_at(0, &mut sb)?;
        if !probe(&sb) {
            return Err(Error::Corrupt);
        }
        let block_size = be32(&sb, 4) as u64;
        let ag_blocks = be32(&sb, 84) as u64;
        let ag_count = be32(&sb, 88) as u64;
        let version = be16(&sb, 100);
        let inode_size = be16(&sb, 104) as u64;
        let inopblock = be16(&sb, 106) as u64;
        let (blocklog, inodelog, inopblog, agblklog) = (sb[120] as u32, sb[122] as u32, sb[123] as u32, sb[124] as u32);
        let dirblklog = sb[192] as u32;
        let features2 = be32(&sb, 200);

        let sane = (9..=16).contains(&blocklog)
            && block_size == 1 << blocklog
            && (8..=11).contains(&inodelog)
            && inode_size == 1 << inodelog
            && inode_size <= block_size
            && inopblock == block_size / inode_size
            && inopblog == blocklog - inodelog
            && (1..=31).contains(&agblklog)
            && ag_blocks > 0
            && ag_blocks <= 1 << agblklog
            && ag_count > 0
            && dirblklog <= 8
            && blocklog + dirblklog <= 16;
        if !sane {
            return Err(Error::Corrupt);
        }
        let (v5, ftype) = match version & 0xf {
            5 => {
                let incompat = be32(&sb, 216);
                if incompat & !KNOWN_INCOMPAT != 0 {
                    return Err(Error::Unsupported);
                }
                (true, incompat & 1 != 0)
            }
            4 => {
                // Version 1 directories predate Linux XFS; refuse them.
                if version & 0x2000 == 0 {
                    return Err(Error::Unsupported);
                }
                let morebits = version & 0x8000 != 0;
                (false, morebits && features2 & 0x200 != 0)
            }
            _ => return Err(Error::Unsupported),
        };
        let label_bytes = &sb[108..120];
        let label_len = label_bytes.iter().position(|&c| c == 0).unwrap_or(12);
        Ok(Self {
            vol,
            block_size,
            ag_blocks,
            ag_count,
            agblklog,
            inopblog,
            inode_size,
            dirblklog,
            root: be64(&sb, 56),
            v5,
            ftype,
            uuid: sb[32..48].try_into().unwrap(),
            label: String::from_utf8_lossy(&label_bytes[..label_len]).into_owned(),
        })
    }

    /// Byte position of a file system block (`| AG | block in AG |`).
    fn fsb_pos(&self, fsb: u64) -> Result<u64> {
        let agno = fsb >> self.agblklog;
        let agbno = fsb & ((1 << self.agblklog) - 1);
        if agno >= self.ag_count || agbno >= self.ag_blocks {
            return Err(Error::Corrupt);
        }
        Ok((agno * self.ag_blocks + agbno) * self.block_size)
    }

    fn inode(&mut self, ino: u64) -> Result<Inode> {
        let shift = self.agblklog + self.inopblog;
        let agno = ino.checked_shr(shift).unwrap_or(0);
        let agbno = (ino >> self.inopblog) & ((1 << self.agblklog) - 1);
        let slot = ino & ((1 << self.inopblog) - 1);
        if agno >= self.ag_count || agbno >= self.ag_blocks {
            return Err(Error::Corrupt);
        }
        let pos = (agno * self.ag_blocks + agbno) * self.block_size + slot * self.inode_size;
        let mut b = vec![0u8; self.inode_size as usize];
        self.vol.read_at(pos, &mut b)?;
        if be16(&b, 0) != 0x494e {
            return Err(Error::Corrupt); // "IN"
        }
        // v3 inodes (v5 file systems) have a 176-byte core, older ones 100.
        let version = b[4];
        let core = if version >= 3 { 176 } else { 100 };
        if b.len() < core {
            return Err(Error::Corrupt);
        }
        let flags2 = if version >= 3 { be64(&b, 120) } else { 0 };
        // With 64-bit extent counts (NREXT64) the data fork count moves
        // into what used to be padding.
        let nextents = if flags2 & (1 << 4) != 0 { be64(&b, 24) } else { be32(&b, 76) as u64 };
        let forkoff = b[82] as usize * 8; // attribute fork offset, 0 = none
        let end = if forkoff != 0 { core + forkoff } else { b.len() };
        if end > b.len() {
            return Err(Error::Corrupt);
        }
        Ok(Inode {
            mode: be16(&b, 2),
            format: b[5],
            size: be64(&b, 56),
            nextents,
            flags: be16(&b, 90),
            fork: b[core..end].to_vec(),
        })
    }

    /// The data fork's extent list (format 2: in the inode; format 3: a
    /// B+tree rooted in the inode).
    fn extents(&mut self, ino: &Inode) -> Result<Vec<Extent>> {
        let f = &ino.fork;
        match ino.format {
            2 => {
                let n = usize::try_from(ino.nextents).map_err(|_| Error::Corrupt)?;
                if n.checked_mul(16).is_none_or(|len| len > f.len()) {
                    return Err(Error::Corrupt);
                }
                Ok((0..n).map(|i| extent_rec(f, i * 16)).collect())
            }
            3 => {
                if f.len() < 4 {
                    return Err(Error::Corrupt);
                }
                let (level, n) = (be16(f, 0), be16(f, 2) as usize);
                // The in-inode root holds as many keys as fit, then the pointers.
                let maxrecs = (f.len() - 4) / 16;
                if level == 0 || level > 10 || n > maxrecs {
                    return Err(Error::Corrupt);
                }
                let mut out = Vec::new();
                let ptrs = 4 + maxrecs * 8;
                for i in 0..n {
                    let ptr = be64(f, ptrs + i * 8);
                    self.bmbt(ptr, level - 1, &mut out)?;
                }
                Ok(out)
            }
            _ => Err(Error::Corrupt),
        }
    }

    fn bmbt(&mut self, fsb: u64, level: u16, out: &mut Vec<Extent>) -> Result<()> {
        let mut b = vec![0u8; self.block_size as usize];
        let pos = self.fsb_pos(fsb)?;
        self.vol.read_at(pos, &mut b)?;
        // "BMAP" (v4, 24-byte header) or "BMA3" (v5, 72 bytes with CRC etc.).
        let (magic, hdr) = if self.v5 { (0x424d_4133, 72) } else { (0x424d_4150, 24) };
        if be32(&b, 0) != magic || be16(&b, 4) != level {
            return Err(Error::Corrupt);
        }
        let n = be16(&b, 6) as usize;
        if level == 0 {
            if hdr + n * 16 > b.len() || out.len() + n > MAX_EXTENTS {
                return Err(Error::Corrupt);
            }
            out.extend((0..n).map(|i| extent_rec(&b, hdr + i * 16)));
            return Ok(());
        }
        let maxrecs = (b.len() - hdr) / 16;
        if n > maxrecs {
            return Err(Error::Corrupt);
        }
        let ptrs = hdr + maxrecs * 8;
        for i in 0..n {
            self.bmbt(be64(&b, ptrs + i * 8), level - 1, out)?;
        }
        Ok(())
    }

    /// Read `count` file blocks from logical block `first`; holes are zeros.
    fn read_blocks(&mut self, exts: &[Extent], first: u64, count: u64) -> Result<Vec<u8>> {
        let bs = self.block_size;
        let mut out = vec![0u8; (count * bs) as usize];
        for e in exts {
            if e.unwritten {
                continue;
            }
            let s = e.off.max(first);
            let t = e.off.saturating_add(e.len).min(first + count);
            if s >= t {
                continue;
            }
            let pos = self.fsb_pos(e.block)? + (s - e.off) * bs;
            let range = ((s - first) * bs) as usize..((t - first) * bs) as usize;
            self.vol.read_at(pos, &mut out[range])?;
        }
        Ok(out)
    }

    /// All entries of a directory as (name, inode, ftype or 0).
    fn dir_entries(&mut self, ino: &Inode) -> Result<Vec<(String, u64, u8)>> {
        if ino.kind() != Kind::Dir {
            return Err(Error::NotDir);
        }
        let ft = self.ftype as usize;
        let mut out = Vec::new();
        if ino.format == 1 {
            // Short form: count, i8count, parent inode, then packed entries
            // (namelen, 2-byte offset, name, [ftype], inode of 4 or 8 bytes).
            let f = &ino.fork;
            if f.len() < 2 {
                return Err(Error::Corrupt);
            }
            let count = f[0] as usize;
            let isz = if f[1] > 0 { 8 } else { 4 };
            let mut p = 2 + isz;
            for _ in 0..count {
                let nl = *f.get(p).ok_or(Error::Corrupt)? as usize;
                let name = f.get(p + 3..p + 3 + nl).ok_or(Error::Corrupt)?;
                let q = p + 3 + nl;
                let ftype = if ft == 1 { *f.get(q).ok_or(Error::Corrupt)? } else { 0 };
                let ib = f.get(q + ft..q + ft + isz).ok_or(Error::Corrupt)?;
                let inum = if isz == 8 { be64(ib, 0) & 0x00ff_ffff_ffff_ffff } else { be32(ib, 0) as u64 };
                out.push((String::from_utf8_lossy(name).into_owned(), inum, ftype));
                p = q + ft + isz;
            }
            return Ok(out);
        }

        // Block, leaf and node directories: the entries are in data blocks
        // (each `1 << dirblklog` file system blocks) below the leaf offset.
        let exts = self.extents(ino)?;
        let dlog = self.dirblklog;
        let limit = XFS_DIR2_LEAF_OFFSET / self.block_size; // in file system blocks
        let mut dblocks = BTreeSet::new();
        for e in &exts {
            if e.unwritten || e.len == 0 || e.off >= limit {
                continue;
            }
            let last = (e.off.saturating_add(e.len) - 1).min(limit - 1);
            for d in (e.off >> dlog)..=(last >> dlog) {
                dblocks.insert(d);
                if dblocks.len() > 1 << 20 {
                    return Err(Error::Corrupt);
                }
            }
        }
        let per = 1u64 << dlog;
        let dbs = (self.block_size << dlog) as usize;
        let hdr = if self.v5 { 64 } else { 16 };
        for d in dblocks {
            let b = self.read_blocks(&exts, d * per, per)?;
            let end = match be32(&b, 0) {
                // Single-block directory ("XD2B"/"XDB3"): a leaf table and an
                // 8-byte tail (count, stale) share the end of the block.
                0x5844_3242 | 0x5844_4233 => {
                    let count = be32(&b, dbs - 8) as usize;
                    dbs.checked_sub(8 + count.checked_mul(8).ok_or(Error::Corrupt)?).ok_or(Error::Corrupt)?
                }
                0x5844_3244 | 0x5844_4433 => dbs, // data block ("XD2D"/"XDD3")
                _ => continue,
            };
            if end < hdr {
                return Err(Error::Corrupt);
            }
            let mut p = hdr;
            while p + 8 <= end {
                if be16(&b, p) == 0xffff {
                    // Unused space: freetag, length.
                    let len = be16(&b, p + 2) as usize;
                    if len < 8 || len % 8 != 0 {
                        return Err(Error::Corrupt);
                    }
                    p += len;
                    continue;
                }
                // inumber, namelen, name, [ftype], 2-byte tag, padded to 8.
                if p + 9 > end {
                    return Err(Error::Corrupt);
                }
                let inum = be64(&b, p);
                let nl = b[p + 8] as usize;
                let size = (8 + 1 + nl + ft + 2 + 7) & !7;
                if nl == 0 || p + size > end {
                    return Err(Error::Corrupt);
                }
                let ftype = if ft == 1 { b[p + 9 + nl] } else { 0 };
                out.push((String::from_utf8_lossy(&b[p + 9..p + 9 + nl]).into_owned(), inum, ftype));
                p += size;
            }
        }
        Ok(out)
    }

    fn entry_kind(&mut self, inum: u64, ftype: u8) -> Result<Kind> {
        Ok(match ftype {
            1 => Kind::File,
            2 => Kind::Dir,
            7 => Kind::Symlink,
            0 => self.inode(inum)?.kind(),
            _ => Kind::Other,
        })
    }
}

impl FileSystem for Xfs {
    fn fs_type(&self) -> FsType {
        FsType::Xfs
    }

    fn uuid(&self) -> String {
        uuid_string(&self.uuid)
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn root(&self) -> u64 {
        self.root
    }

    fn lookup(&mut self, dir: u64, name: &str) -> Result<(u64, Kind)> {
        let ino = self.inode(dir)?;
        let entries = self.dir_entries(&ino)?;
        let (_, inum, ftype) = entries.into_iter().find(|e| e.0 == name).ok_or(Error::NotFound)?;
        Ok((inum, self.entry_kind(inum, ftype)?))
    }

    fn list(&mut self, dir: u64) -> Result<Vec<DirEntry>> {
        let ino = self.inode(dir)?;
        let entries = self.dir_entries(&ino)?;
        let mut out = Vec::with_capacity(entries.len());
        for (name, inum, ftype) in entries {
            if name == "." || name == ".." {
                continue;
            }
            let kind = self.entry_kind(inum, ftype)?;
            out.push(DirEntry { name, kind });
        }
        Ok(out)
    }

    fn file_size(&mut self, node: u64) -> Result<u64> {
        Ok(self.inode(node)?.size)
    }

    fn read(&mut self, node: u64, limit: u64) -> Result<Vec<u8>> {
        let ino = self.inode(node)?;
        if ino.kind() != Kind::File {
            return Err(Error::NotFile);
        }
        if ino.flags & 1 != 0 {
            return Err(Error::Unsupported); // data on the realtime device
        }
        if ino.size > limit {
            return Err(Error::TooBig);
        }
        let size = ino.size as usize;
        let mut data = vec![0u8; size];
        match ino.format {
            1 => {
                let n = size.min(ino.fork.len());
                data[..n].copy_from_slice(&ino.fork[..n]);
            }
            2 | 3 => {
                let bs = self.block_size;
                for e in self.extents(&ino)? {
                    if e.unwritten {
                        continue;
                    }
                    let start = e.off.checked_mul(bs).ok_or(Error::Corrupt)?;
                    if start >= ino.size {
                        continue;
                    }
                    let len = e.len.checked_mul(bs).ok_or(Error::Corrupt)?.min(ino.size - start);
                    let pos = self.fsb_pos(e.block)?;
                    self.vol.read_at(pos, &mut data[start as usize..(start + len) as usize])?;
                }
            }
            _ => return Err(Error::Corrupt),
        }
        Ok(data)
    }

    fn read_link(&mut self, node: u64) -> Result<String> {
        let ino = self.inode(node)?;
        if ino.kind() != Kind::Symlink {
            return Err(Error::NotFile);
        }
        let size = ino.size as usize;
        if size > 1024 {
            return Err(Error::Corrupt);
        }
        let target = match ino.format {
            1 => ino.fork.get(..size).ok_or(Error::Corrupt)?.to_vec(),
            2 | 3 => {
                // Remote symlink: on v5 each block starts with a 56-byte
                // header ("XSLM", offset, bytes, crc, uuid, owner, blkno, lsn).
                let exts = self.extents(&ino)?;
                let hdr = if self.v5 { 56 } else { 0 };
                let per_block = self.block_size as usize - hdr;
                let blocks = size.div_ceil(per_block) as u64;
                let raw = self.read_blocks(&exts, 0, blocks)?;
                let mut t = Vec::with_capacity(size);
                for chunk in raw.chunks(self.block_size as usize) {
                    if self.v5 && be32(chunk, 0) != 0x5853_4c4d {
                        return Err(Error::Corrupt);
                    }
                    let take = (size - t.len()).min(per_block);
                    t.extend_from_slice(&chunk[hdr..hdr + take]);
                }
                t
            }
            _ => return Err(Error::Corrupt),
        };
        Ok(String::from_utf8_lossy(&target).into_owned())
    }
}
