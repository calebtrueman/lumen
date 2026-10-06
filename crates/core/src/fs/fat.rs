//! FAT12/16/32 reader (read-only), with long file names. Names are matched
//! case-insensitively, as FAT itself does. FAT has no symlinks.
//!
//! Node handles are indexes into a table of the files and directories seen
//! so far (FAT has no inode numbers); handle 0 is the root directory.

use super::*;

/// `block` is the first 4 KiB of the volume.
pub fn probe(block: &[u8]) -> bool {
    if block.len() < 512 || block[510] != 0x55 || block[511] != 0xAA {
        return false;
    }
    let bps = le16(block, 11);
    let spc = block[13];
    // A plausible BPB plus a FAT type string is what blkid looks for too.
    matches!(bps, 512 | 1024 | 2048 | 4096)
        && spc.is_power_of_two()
        && le16(block, 14) != 0
        && block[16] != 0
        && (&block[54..57] == b"FAT" || &block[82..87] == b"FAT32" || &block[54..59] == b"MSDOS")
}

#[derive(Clone, Copy, PartialEq)]
enum Bits {
    Fat12,
    Fat16,
    Fat32,
}

#[derive(Clone, Copy)]
struct Node {
    cluster: u32,
    size: u32,
    dir: bool,
}

pub struct Fat {
    vol: Cache,
    bits: Bits,
    bytes_per_sector: u64,
    cluster_size: u64,
    fat_offset: u64,
    /// FAT12/16: the fixed-size root directory region.
    root_offset: u64,
    root_entries: u64,
    data_offset: u64,
    clusters: u32,
    root_cluster: u32,
    serial: u32,
    label: String,
    nodes: Vec<Node>,
}

impl Fat {
    pub fn open(mut vol: Cache) -> Result<Self> {
        let mut b = [0u8; 512];
        vol.read_at(0, &mut b)?;
        let bps = le16(&b, 11) as u64;
        let spc = b[13] as u64;
        let reserved = le16(&b, 14) as u64;
        let nfats = b[16] as u64;
        let root_entries = le16(&b, 17) as u64;
        let total = match le16(&b, 19) {
            0 => le32(&b, 32) as u64,
            n => n as u64,
        };
        let fat_size = match le16(&b, 22) {
            0 => le32(&b, 36) as u64,
            n => n as u64,
        };
        if bps == 0 || spc == 0 || fat_size == 0 {
            return Err(Error::Corrupt);
        }
        let root_sectors = (root_entries * 32).div_ceil(bps);
        let data_start = reserved + nfats * fat_size + root_sectors;
        let clusters = (total.saturating_sub(data_start) / spc) as u32;
        // The FAT type is decided by the cluster count alone.
        let bits = if clusters < 4085 {
            Bits::Fat12
        } else if clusters < 65525 {
            Bits::Fat16
        } else {
            Bits::Fat32
        };
        let (serial_at, label_at) = if bits == Bits::Fat32 { (67, 71) } else { (39, 43) };
        let mut fat = Self {
            vol,
            bits,
            bytes_per_sector: bps,
            cluster_size: bps * spc,
            fat_offset: reserved * bps,
            root_offset: (reserved + nfats * fat_size) * bps,
            root_entries,
            data_offset: data_start * bps,
            clusters,
            root_cluster: if bits == Bits::Fat32 { le32(&b, 44) } else { 0 },
            serial: le32(&b, serial_at),
            label: trim_name(&b[label_at..label_at + 11]),
            nodes: alloc::vec![Node { cluster: 0, size: 0, dir: true }],
        };
        fat.nodes[0].cluster = fat.root_cluster;
        // The volume label entry in the root directory wins over the BPB's
        // copy (that's what Windows and blkid show).
        if let Ok(raw) = fat.dir_bytes(0) {
            for e in raw.chunks_exact(32) {
                if e[0] == 0 {
                    break;
                }
                if e[0] != 0xE5 && e[11] & 0x08 != 0 && e[11] & 0x0F != 0x0F {
                    fat.label = trim_name(&e[0..11]);
                    break;
                }
            }
        }
        if fat.label == "NO NAME" {
            fat.label.clear();
        }
        Ok(fat)
    }

    fn next_cluster(&mut self, c: u32) -> Result<Option<u32>> {
        let (off, eoc) = match self.bits {
            Bits::Fat12 => (c as u64 * 3 / 2, 0xFF8),
            Bits::Fat16 => (c as u64 * 2, 0xFFF8),
            Bits::Fat32 => (c as u64 * 4, 0x0FFF_FFF8),
        };
        let mut b = [0u8; 4];
        let n = if self.bits == Bits::Fat32 { 4 } else { 2 };
        self.vol.read_at(self.fat_offset + off, &mut b[..n])?;
        let v = match self.bits {
            Bits::Fat12 => {
                let v = le16(&b, 0) as u32;
                if c & 1 == 1 { v >> 4 } else { v & 0xFFF }
            }
            Bits::Fat16 => le16(&b, 0) as u32,
            Bits::Fat32 => le32(&b, 0) & 0x0FFF_FFFF,
        };
        Ok(if v >= eoc || v < 2 { None } else { Some(v) })
    }

    /// Byte ranges of a cluster chain, merging runs of adjacent clusters.
    fn chain(&mut self, first: u32, max_bytes: u64) -> Result<Vec<(u64, u64)>> {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut c = first;
        let mut total = 0u64;
        let mut steps = 0u32;
        while c >= 2 && total < max_bytes {
            if c - 2 >= self.clusters || steps > self.clusters {
                return Err(Error::Corrupt);
            }
            steps += 1;
            let off = self.data_offset + (c - 2) as u64 * self.cluster_size;
            match runs.last_mut() {
                Some(r) if r.0 + r.1 == off => r.1 += self.cluster_size,
                _ => runs.push((off, self.cluster_size)),
            }
            total += self.cluster_size;
            match self.next_cluster(c)? {
                Some(n) => c = n,
                None => break,
            }
        }
        Ok(runs)
    }

    fn dir_bytes(&mut self, node: u64) -> Result<Vec<u8>> {
        let n = *self.nodes.get(node as usize).ok_or(Error::Corrupt)?;
        if !n.dir {
            return Err(Error::NotDir);
        }
        if node == 0 && self.bits != Bits::Fat32 {
            let mut buf = alloc::vec![0u8; (self.root_entries * 32) as usize];
            self.vol.read_at(self.root_offset, &mut buf)?;
            return Ok(buf);
        }
        let mut buf = Vec::new();
        for (off, len) in self.chain(n.cluster, 64 << 20)? {
            let at = buf.len();
            buf.resize(at + len as usize, 0);
            self.vol.read_at(off, &mut buf[at..])?;
        }
        Ok(buf)
    }

    /// Directory entries as (name, cluster, size, is_dir).
    fn entries(&mut self, node: u64) -> Result<Vec<(String, u32, u32, bool)>> {
        let raw = self.dir_bytes(node)?;
        let mut out = Vec::new();
        let mut lfn: Vec<u16> = Vec::new();
        let mut lfn_parts = 0u8;
        for e in raw.chunks_exact(32) {
            match e[0] {
                0 => break,
                0xE5 => {
                    lfn.clear();
                    continue;
                }
                _ => {}
            }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                // Long name pieces come in reverse order before the 8.3 entry.
                let seq = e[0] & 0x1F;
                if e[0] & 0x40 != 0 {
                    lfn.clear();
                    lfn_parts = seq;
                }
                let mut part = [0u16; 13];
                for (i, o) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                    part[i] = le16(e, *o);
                }
                let mut piece: Vec<u16> = part.iter().copied().take_while(|&c| c != 0 && c != 0xFFFF).collect();
                piece.extend_from_slice(&lfn);
                lfn = piece;
                let _ = lfn_parts;
                continue;
            }
            if attr & 0x08 != 0 {
                lfn.clear();
                continue; // volume label
            }
            let name = if !lfn.is_empty() {
                String::from_utf16_lossy(&lfn)
            } else {
                short_name(e)
            };
            lfn.clear();
            if name == "." || name == ".." {
                continue;
            }
            let hi = if self.bits == Bits::Fat32 { (le16(e, 20) as u32) << 16 } else { 0 };
            out.push((name, hi | le16(e, 26) as u32, le32(e, 28), attr & 0x10 != 0));
        }
        Ok(out)
    }

    fn node(&mut self, n: Node) -> u64 {
        if let Some(i) = self.nodes.iter().position(|x| x.cluster == n.cluster && x.cluster != 0 && x.dir == n.dir) {
            self.nodes[i].size = n.size;
            return i as u64;
        }
        self.nodes.push(n);
        (self.nodes.len() - 1) as u64
    }
}

/// An 8.3 name, honouring the lowercase flags Windows NT stores in byte 12.
fn short_name(e: &[u8]) -> String {
    let lower_base = e[12] & 0x08 != 0;
    let lower_ext = e[12] & 0x10 != 0;
    let mut first = e[0];
    if first == 0x05 {
        first = 0xE5;
    }
    let mut base: Vec<u8> = core::iter::once(first).chain(e[1..8].iter().copied()).collect();
    while base.last() == Some(&b' ') {
        base.pop();
    }
    let mut ext: Vec<u8> = e[8..11].to_vec();
    while ext.last() == Some(&b' ') {
        ext.pop();
    }
    let conv = |v: Vec<u8>, lower: bool| -> String {
        v.iter().map(|&c| if lower { (c as char).to_ascii_lowercase() } else { c as char }).collect()
    };
    let mut s = conv(base, lower_base);
    if !ext.is_empty() {
        s.push('.');
        s.push_str(&conv(ext, lower_ext));
    }
    s
}

fn trim_name(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_end().into()
}

impl FileSystem for Fat {
    fn fs_type(&self) -> FsType {
        FsType::Fat
    }
    /// FAT has a 32-bit volume serial instead, shown as "ABCD-1234".
    fn uuid(&self) -> String {
        let hex = b"0123456789ABCDEF";
        let mut s = String::new();
        for i in (0..8).rev() {
            if i == 3 {
                s.push('-');
            }
            s.push(hex[(self.serial >> (i * 4) & 15) as usize] as char);
        }
        s
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn root(&self) -> u64 {
        0
    }
    fn lookup(&mut self, dir: u64, name: &str) -> Result<(u64, Kind)> {
        let (_, cluster, size, is_dir) =
            self.entries(dir)?.into_iter().find(|e| e.0.eq_ignore_ascii_case(name)).ok_or(Error::NotFound)?;
        // A ".." entry pointing at cluster 0 means the root; ".." isn't
        // returned by entries(), and resolve() tracks parents itself.
        let n = self.node(Node { cluster, size, dir: is_dir });
        Ok((n, if is_dir { Kind::Dir } else { Kind::File }))
    }
    fn list(&mut self, dir: u64) -> Result<Vec<DirEntry>> {
        Ok(self
            .entries(dir)?
            .into_iter()
            .map(|(name, _, _, is_dir)| DirEntry { name, kind: if is_dir { Kind::Dir } else { Kind::File } })
            .collect())
    }
    fn file_size(&mut self, node: u64) -> Result<u64> {
        Ok(self.nodes.get(node as usize).ok_or(Error::Corrupt)?.size as u64)
    }
    fn read(&mut self, node: u64, limit: u64) -> Result<Vec<u8>> {
        let n = *self.nodes.get(node as usize).ok_or(Error::Corrupt)?;
        if n.dir {
            return Err(Error::NotFile);
        }
        if n.size as u64 > limit {
            return Err(Error::TooBig);
        }
        let mut data = alloc::vec![0u8; n.size as usize];
        let mut at = 0usize;
        for (off, len) in self.chain(n.cluster, n.size as u64)? {
            let take = (len as usize).min(data.len() - at);
            self.vol.read_at(off, &mut data[at..at + take])?;
            at += take;
        }
        if at < data.len() {
            return Err(Error::Corrupt);
        }
        let _ = self.bytes_per_sector;
        Ok(data)
    }
    fn read_link(&mut self, _node: u64) -> Result<String> {
        Err(Error::NotFound)
    }
}
