//! btrfs reader (read-only, single-device).
//!
//! Everything in btrfs lives in B-trees keyed by (objectid, type, offset).
//! Tree blocks are addressed by *logical* bytenr, mapped to disk offsets
//! by the chunk tree (bootstrapped from the superblock's sys_chunk_array).
//! The root tree holds one ROOT_ITEM per subvolume; each subvolume is its
//! own file tree with the root directory at inode 256.
//!
//! Like GRUB, "/" is the *default* subvolume (openSUSE sets it to the
//! current snapshot); other subvolumes are reached through their directory
//! entries, e.g. Ubuntu's "/@/boot".
//!
//! Node handles pack (tree id, inode): tree in the top 24 bits, inode in
//! the low 40. Subvolume ids and inode numbers beyond that are refused.

use super::*;

const SUPER_OFFSET: u64 = 0x10000;
const HEADER: usize = 101;

// Item types.
const INODE_ITEM: u8 = 1;
const DIR_ITEM: u8 = 84;
const DIR_INDEX: u8 = 96;
const EXTENT_DATA: u8 = 108;
const ROOT_ITEM: u8 = 132;
const CHUNK_ITEM: u8 = 228;

const FS_TREE: u64 = 5;
const FIRST_FREE: u64 = 256;

// Chunk profiles that spread data over several stripes.
const STRIPED: u64 = 0x08 | 0x40 | 0x80 | 0x100; // RAID0, RAID10, RAID5, RAID6

/// Caps that keep a corrupt image from looping or exhausting memory.
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 200_000;
const MAX_EXTENT: u64 = 1 << 20; // compressed extents are at most 128 KiB

pub fn probe(block: &[u8]) -> bool {
    block.len() >= 0x48 && &block[0x40..0x48] == b"_BHRfS_M"
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Key {
    objectid: u64,
    ty: u8,
    offset: u64,
}

impl Key {
    fn new(objectid: u64, ty: u8, offset: u64) -> Self {
        Self { objectid, ty, offset }
    }
    fn parse(b: &[u8]) -> Self {
        Self { objectid: le64(b, 0), ty: b[8], offset: le64(b, 9) }
    }
}

struct Chunk {
    logical: u64,
    length: u64,
    physical: u64,
}

pub struct Btrfs {
    vol: Cache,
    nodesize: usize,
    sectorsize: u64,
    chunks: Vec<Chunk>,
    /// Root tree block and level.
    root_tree: (u64, u8),
    /// Tree id of the default subvolume ("/").
    default_tree: u64,
    uuid: String,
    label: String,
    nodes_read: usize,
}

fn handle(tree: u64, ino: u64) -> Result<u64> {
    if tree >= 1 << 24 || ino >= 1 << 40 {
        return Err(Error::Unsupported);
    }
    Ok(tree << 40 | ino)
}

fn unhandle(h: u64) -> (u64, u64) {
    (h >> 40, h & ((1 << 40) - 1))
}

fn kind_of(ty: u8) -> Kind {
    match ty {
        1 => Kind::File,
        2 => Kind::Dir,
        7 => Kind::Symlink,
        _ => Kind::Other,
    }
}

/// btrfs's name hash: CRC-32C seeded with ~1, without the final inversion.
fn name_hash(name: &[u8]) -> u32 {
    let mut crc = !1u32;
    for &b in name {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
        }
    }
    crc
}

fn get(b: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    b.get(start..start.checked_add(len).ok_or(Error::Corrupt)?).ok_or(Error::Corrupt)
}

/// One directory entry as stored in DIR_ITEM / DIR_INDEX items (several
/// may be packed into one DIR_ITEM when names collide).
struct DirRec<'a> {
    location: Key,
    ty: u8,
    name: &'a [u8],
}

fn dir_records(data: &[u8]) -> Result<Vec<DirRec<'_>>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < data.len() {
        let h = get(data, p, 30)?;
        let (data_len, name_len) = (le16(h, 25) as usize, le16(h, 27) as usize);
        let name = get(data, p + 30, name_len)?;
        out.push(DirRec { location: Key::parse(h), ty: h[29], name });
        p += 30 + name_len + data_len;
    }
    Ok(out)
}

impl Btrfs {
    pub fn open(mut vol: Cache) -> Result<Self> {
        let mut sb = alloc::vec![0u8; 4096];
        vol.read_at(SUPER_OFFSET, &mut sb)?;
        if !probe(&sb) {
            return Err(Error::Corrupt);
        }
        if le64(&sb, 0x88) != 1 {
            return Err(Error::Unsupported);
        }
        let nodesize = le32(&sb, 0x94) as usize;
        let sectorsize = le32(&sb, 0x90) as u64;
        if !(4096..=65536).contains(&nodesize) || !nodesize.is_power_of_two() || !(512..=65536).contains(&sectorsize) {
            return Err(Error::Corrupt);
        }
        let label_bytes = &sb[0x12b..0x12b + 256];
        let label_len = label_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        let mut fs = Self {
            vol,
            nodesize,
            sectorsize,
            chunks: Vec::new(),
            root_tree: (le64(&sb, 0x50), sb[0xc6]),
            default_tree: FS_TREE,
            uuid: uuid_string(&sb[0x20..0x30]),
            label: String::from_utf8_lossy(&label_bytes[..label_len]).into_owned(),
            nodes_read: 0,
        };

        // Bootstrap: the chunks holding the chunk tree itself.
        let n = (le32(&sb, 0xa0) as usize).min(2048);
        let arr = get(&sb, 0x32b, n)?.to_vec();
        let mut p = 0;
        while p + 17 <= arr.len() {
            let key = Key::parse(&arr[p..]);
            if key.ty != CHUNK_ITEM {
                return Err(Error::Corrupt);
            }
            let size = fs.add_chunk(key.offset, &arr[p + 17..])?;
            p += 17 + size;
        }

        // Then every chunk, from the chunk tree.
        let chunk_root = (le64(&sb, 0x58), sb[0xc7]);
        let mut items = Vec::new();
        fs.search(chunk_root.0, Key::new(256, CHUNK_ITEM, 0), Key::new(256, CHUNK_ITEM, u64::MAX), &mut items, 0)?;
        for (key, data) in items {
            if !fs.chunks.iter().any(|c| c.logical == key.offset) {
                fs.add_chunk(key.offset, &data)?;
            }
        }

        // The default subvolume: dir item "default" in the root tree's
        // directory (superblock root_dir_objectid, normally 6).
        let root_dir = le64(&sb, 0x80);
        let hash = name_hash(b"default") as u64;
        let mut items = Vec::new();
        fs.search(fs.root_tree.0, Key::new(root_dir, DIR_ITEM, hash), Key::new(root_dir, DIR_ITEM, hash), &mut items, 0)?;
        for (_, data) in &items {
            for rec in dir_records(data)? {
                if rec.name == b"default" && rec.location.ty == ROOT_ITEM {
                    fs.default_tree = rec.location.objectid;
                }
            }
        }
        // Fails early (Unsupported) if the default subvolume id doesn't fit a handle.
        handle(fs.default_tree, FIRST_FREE)?;
        Ok(fs)
    }

    /// Parse a chunk item and record its first stripe; returns its size.
    fn add_chunk(&mut self, logical: u64, item: &[u8]) -> Result<usize> {
        let h = get(item, 0, 48)?;
        let (length, ty, stripes) = (le64(h, 0), le64(h, 24), le16(h, 44) as usize);
        let size = 48 + stripes * 32;
        get(item, 0, size)?;
        if stripes == 0 {
            return Err(Error::Corrupt);
        }
        if ty & STRIPED != 0 && stripes > 1 {
            return Err(Error::Unsupported);
        }
        // SINGLE, DUP (and mirrored copies): the first stripe holds it all.
        let physical = le64(item, 48 + 8);
        self.chunks.push(Chunk { logical, length, physical });
        Ok(size)
    }

    fn map(&self, logical: u64) -> Result<(u64, u64)> {
        let c = self
            .chunks
            .iter()
            .find(|c| logical >= c.logical && logical - c.logical < c.length)
            .ok_or(Error::Corrupt)?;
        Ok((c.physical + (logical - c.logical), c.length - (logical - c.logical)))
    }

    fn read_logical(&mut self, mut logical: u64, buf: &mut [u8]) -> Result<()> {
        let mut done = 0;
        while done < buf.len() {
            let (phys, avail) = self.map(logical)?;
            let take = (buf.len() - done).min(avail.min(usize::MAX as u64) as usize);
            self.vol.read_at(phys, &mut buf[done..done + take])?;
            done += take;
            logical += take as u64;
        }
        Ok(())
    }

    fn read_node(&mut self, bytenr: u64) -> Result<Vec<u8>> {
        self.nodes_read += 1;
        if self.nodes_read > MAX_NODES {
            return Err(Error::Corrupt);
        }
        let mut node = alloc::vec![0u8; self.nodesize];
        self.read_logical(bytenr, &mut node)?;
        if le64(&node, 48) != bytenr {
            return Err(Error::Corrupt);
        }
        Ok(node)
    }

    /// Collect every item with lo <= key <= hi from the tree at `bytenr`.
    fn search(&mut self, bytenr: u64, lo: Key, hi: Key, out: &mut Vec<(Key, Vec<u8>)>, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::Corrupt);
        }
        if depth == 0 {
            self.nodes_read = 0;
        }
        let node = self.read_node(bytenr)?;
        let nritems = le32(&node, 96) as usize;
        let level = node[100];
        if level == 0 {
            for i in 0..nritems {
                let it = get(&node, HEADER + i * 25, 25)?;
                let key = Key::parse(it);
                if key < lo {
                    continue;
                }
                if key > hi {
                    break;
                }
                let (off, size) = (le32(it, 17) as usize, le32(it, 21) as usize);
                out.push((key, get(&node, HEADER + off, size)?.to_vec()));
            }
        } else {
            let ptr = |i: usize| -> Result<(Key, u64)> {
                let p = get(&node, HEADER + i * 33, 33)?;
                Ok((Key::parse(p), le64(p, 17)))
            };
            for i in 0..nritems {
                let (key, child) = ptr(i)?;
                if key > hi {
                    break;
                }
                // Child i covers [key_i, key_{i+1}).
                if i + 1 < nritems && ptr(i + 1)?.0 <= lo {
                    continue;
                }
                self.search(child, lo, hi, out, depth + 1)?;
            }
        }
        Ok(())
    }

    /// The root block of a subvolume's file tree.
    fn tree_root(&mut self, tree: u64) -> Result<u64> {
        let mut items = Vec::new();
        let root = self.root_tree.0;
        self.search(root, Key::new(tree, ROOT_ITEM, 0), Key::new(tree, ROOT_ITEM, u64::MAX), &mut items, 0)?;
        let (_, item) = items.last().ok_or(Error::NotFound)?;
        Ok(le64(get(item, 176, 8)?, 0))
    }

    fn items(&mut self, tree: u64, lo: Key, hi: Key) -> Result<Vec<(Key, Vec<u8>)>> {
        let root = self.tree_root(tree)?;
        let mut out = Vec::new();
        self.search(root, lo, hi, &mut out, 0)?;
        Ok(out)
    }

    fn inode(&mut self, tree: u64, ino: u64) -> Result<Vec<u8>> {
        let mut items = self.items(tree, Key::new(ino, INODE_ITEM, 0), Key::new(ino, INODE_ITEM, 0))?;
        let (_, data) = items.pop().ok_or(Error::NotFound)?;
        get(&data, 0, 160)?;
        Ok(data)
    }

    /// Where a directory entry points: into the same tree, or into the
    /// root directory of another subvolume.
    fn target(&self, tree: u64, rec: &DirRec) -> Result<(u64, Kind)> {
        if rec.location.ty == ROOT_ITEM {
            return Ok((handle(rec.location.objectid, FIRST_FREE)?, Kind::Dir));
        }
        Ok((handle(tree, rec.location.objectid)?, kind_of(rec.ty)))
    }

    fn decompress(&self, method: u8, data: &[u8], size: u64) -> Result<Vec<u8>> {
        if size > MAX_EXTENT {
            return Err(Error::Corrupt);
        }
        let size = size as usize;
        match method {
            1 => match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, size) {
                Ok(v) => Ok(v),
                Err(_) => Err(Error::Corrupt),
            },
            2 => lzo_segments(data, size, self.sectorsize as usize),
            3 => zstd(data, size),
            _ => Err(Error::Unsupported),
        }
    }
}

/// btrfs zstd extents are one frame, followed by zero padding to the sector.
fn zstd(data: &[u8], size: usize) -> Result<Vec<u8>> {
    use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
    use ruzstd::io::Read;
    let mut dec = FrameDecoder::new();
    let mut src = data;
    dec.init(&mut src).map_err(|_| Error::Corrupt)?;
    let mut out = alloc::vec![0u8; size];
    let mut pos = 0;
    for _ in 0..4096 {
        dec.decode_blocks(&mut src, BlockDecodingStrategy::UptoBytes(64 * 1024)).map_err(|_| Error::Corrupt)?;
        if dec.can_collect() > size - pos {
            return Err(Error::Corrupt);
        }
        pos += dec.read(&mut out[pos..]).map_err(|_| Error::Corrupt)?;
        if dec.is_finished() {
            if dec.can_collect() > 0 {
                return Err(Error::Corrupt);
            }
            out.truncate(pos);
            return Ok(out);
        }
    }
    Err(Error::Corrupt)
}

/// btrfs's LZO framing: a 4-byte total length, then segments of
/// (4-byte length, LZO1X data for at most one sector). A segment header
/// never straddles a sector boundary; if fewer than 4 bytes remain in the
/// sector, they are padding.
fn lzo_segments(data: &[u8], size: usize, sector: usize) -> Result<Vec<u8>> {
    let total = (le32(get(data, 0, 4)?, 0) as usize).min(data.len());
    let mut out = Vec::with_capacity(size);
    let mut p = 4;
    while p < total && out.len() < size {
        if sector - p % sector < 4 {
            p += sector - p % sector;
            continue;
        }
        let len = le32(get(data, p, 4)?, 0) as usize;
        p += 4;
        let seg = get(data, p, len)?;
        lzo1x(seg, &mut out, size)?;
        p += len;
    }
    Ok(out)
}

/// LZO1X decompression (the format of the kernel's lzo1x_decompress_safe),
/// appending to `out` but never growing it past `limit`.
fn lzo1x(src: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<()> {
    let mut ip = 0usize;
    let byte = |ip: &mut usize| -> Result<usize> {
        let b = *src.get(*ip).ok_or(Error::Corrupt)?;
        *ip += 1;
        Ok(b as usize)
    };
    // Lengths of 0 extend with zero bytes (255 each) and a final byte.
    let run = |ip: &mut usize, base: usize| -> Result<usize> {
        let mut n = 0usize;
        while *src.get(*ip).ok_or(Error::Corrupt)? == 0 {
            *ip += 1;
            n += 255;
            if n > 1 << 24 {
                return Err(Error::Corrupt);
            }
        }
        Ok(n + base + byte(ip)?)
    };
    let literals = |ip: &mut usize, out: &mut Vec<u8>, n: usize| -> Result<()> {
        if out.len() + n > limit {
            return Err(Error::Corrupt);
        }
        out.extend_from_slice(get(src, *ip, n)?);
        *ip += n;
        Ok(())
    };
    let copy = |out: &mut Vec<u8>, dist: usize, n: usize| -> Result<()> {
        if dist == 0 || dist > out.len() || out.len() + n > limit {
            return Err(Error::Corrupt);
        }
        let start = out.len() - dist;
        for i in 0..n {
            let b = out[start + i];
            out.push(b);
        }
        Ok(())
    };

    let mut state;
    let first = *src.first().ok_or(Error::Corrupt)? as usize;
    if first > 17 {
        ip = 1;
        let t = first - 17;
        literals(&mut ip, out, t)?;
        state = if t < 4 { t } else { 4 };
    } else {
        state = 0;
    }
    loop {
        let t = byte(&mut ip)?;
        let next;
        if t < 16 {
            if state == 0 {
                // A run of literals.
                let n = if t == 0 { run(&mut ip, 15)? } else { t } + 3;
                literals(&mut ip, out, n)?;
                state = 4;
                continue;
            }
            next = t & 3;
            let b = byte(&mut ip)?;
            if state != 4 {
                // 2-byte match close behind.
                copy(out, 1 + (t >> 2) + (b << 2), 2)?;
            } else {
                // 3-byte match just past the 2 KiB window.
                copy(out, 1 + 0x800 + (t >> 2) + (b << 2), 3)?;
            }
        } else if t >= 64 {
            next = t & 3;
            let b = byte(&mut ip)?;
            copy(out, 1 + ((t >> 2) & 7) + (b << 3), (t >> 5) + 1)?;
        } else if t >= 32 {
            let n = if t & 31 == 0 { run(&mut ip, 31)? } else { t & 31 } + 2;
            let d = le16(get(src, ip, 2)?, 0) as usize;
            ip += 2;
            next = d & 3;
            copy(out, 1 + (d >> 2), n)?;
        } else {
            let n = if t & 7 == 0 { run(&mut ip, 7)? } else { t & 7 } + 2;
            let d = le16(get(src, ip, 2)?, 0) as usize;
            ip += 2;
            next = d & 3;
            let dist = ((t & 8) << 11) + (d >> 2);
            if dist == 0 {
                return Ok(()); // end of stream
            }
            copy(out, dist + 0x4000, n)?;
        }
        literals(&mut ip, out, next)?;
        state = next;
    }
}

impl FileSystem for Btrfs {
    fn fs_type(&self) -> FsType {
        FsType::Btrfs
    }
    fn uuid(&self) -> String {
        self.uuid.clone()
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn root(&self) -> u64 {
        handle(self.default_tree, FIRST_FREE).unwrap_or(0)
    }

    fn lookup(&mut self, dir: u64, name: &str) -> Result<(u64, Kind)> {
        let (tree, ino) = unhandle(dir);
        let hash = name_hash(name.as_bytes()) as u64;
        let items = self.items(tree, Key::new(ino, DIR_ITEM, hash), Key::new(ino, DIR_ITEM, hash))?;
        for (_, data) in &items {
            for rec in dir_records(data)? {
                if rec.name == name.as_bytes() {
                    return self.target(tree, &rec);
                }
            }
        }
        Err(Error::NotFound)
    }

    fn list(&mut self, dir: u64) -> Result<Vec<DirEntry>> {
        let (tree, ino) = unhandle(dir);
        let mode = le32(&self.inode(tree, ino)?, 52);
        if mode & 0o170000 != 0o040000 {
            return Err(Error::NotDir);
        }
        let items = self.items(tree, Key::new(ino, DIR_INDEX, 0), Key::new(ino, DIR_INDEX, u64::MAX))?;
        let mut out = Vec::with_capacity(items.len());
        for (_, data) in &items {
            for rec in dir_records(data)? {
                let kind = if rec.location.ty == ROOT_ITEM { Kind::Dir } else { kind_of(rec.ty) };
                out.push(DirEntry { name: String::from_utf8_lossy(rec.name).into_owned(), kind });
            }
        }
        Ok(out)
    }

    fn file_size(&mut self, node: u64) -> Result<u64> {
        let (tree, ino) = unhandle(node);
        Ok(le64(&self.inode(tree, ino)?, 16))
    }

    fn read(&mut self, node: u64, limit: u64) -> Result<Vec<u8>> {
        let (tree, ino) = unhandle(node);
        let size = le64(&self.inode(tree, ino)?, 16);
        if size > limit || size > usize::MAX as u64 {
            return Err(Error::TooBig);
        }
        let mut out = alloc::vec![0u8; size as usize];
        let items = self.items(tree, Key::new(ino, EXTENT_DATA, 0), Key::new(ino, EXTENT_DATA, u64::MAX))?;
        for (key, item) in items {
            let file_off = key.offset;
            if file_off >= size {
                continue;
            }
            let h = get(&item, 0, 21)?;
            let (ram_bytes, compression, encryption, ty) = (le64(h, 8), h[16], h[17], h[20]);
            if encryption != 0 || le16(h, 18) != 0 {
                return Err(Error::Unsupported);
            }
            let room = (size - file_off) as usize;
            let dst = &mut out[file_off as usize..];
            match ty {
                0 => {
                    let inline = &item[21..];
                    if compression == 0 {
                        let n = inline.len().min(room);
                        dst[..n].copy_from_slice(&inline[..n]);
                    } else {
                        let data = self.decompress(compression, inline, ram_bytes)?;
                        let n = data.len().min(room);
                        dst[..n].copy_from_slice(&data[..n]);
                    }
                }
                1 => {
                    let r = get(&item, 21, 32)?;
                    let (disk_bytenr, disk_bytes, offset, num_bytes) = (le64(r, 0), le64(r, 8), le64(r, 16), le64(r, 24));
                    if disk_bytenr == 0 {
                        continue; // hole
                    }
                    let n = (num_bytes.min(room as u64)) as usize;
                    if compression == 0 {
                        self.read_logical(disk_bytenr + offset, &mut dst[..n])?;
                    } else {
                        if disk_bytes > MAX_EXTENT {
                            return Err(Error::Corrupt);
                        }
                        let mut raw = alloc::vec![0u8; disk_bytes as usize];
                        self.read_logical(disk_bytenr, &mut raw)?;
                        let data = self.decompress(compression, &raw, ram_bytes)?;
                        let src = data.get(offset as usize..).ok_or(Error::Corrupt)?;
                        let n = n.min(src.len());
                        dst[..n].copy_from_slice(&src[..n]);
                    }
                }
                2 => {} // preallocated: reads as zeros
                _ => return Err(Error::Corrupt),
            }
        }
        Ok(out)
    }

    fn read_link(&mut self, node: u64) -> Result<String> {
        let (tree, ino) = unhandle(node);
        let mode = le32(&self.inode(tree, ino)?, 52);
        if mode & 0o170000 != 0o120000 {
            return Err(Error::NotFile);
        }
        let target = self.read(node, 4096)?;
        Ok(String::from_utf8_lossy(&target).into_owned())
    }
}
