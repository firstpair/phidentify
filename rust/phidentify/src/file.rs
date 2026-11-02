//! Whole-file byte identity and the stat stamp that guards it.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// What must not change while a file is read. Device is kept for reporting but
/// excluded from equality, because it changes when a volume is remounted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub inode: u64,
    pub device: u64,
}

impl PartialEq for Stamp {
    fn eq(&self, o: &Self) -> bool {
        (self.size, self.mtime_ns, self.ctime_ns, self.inode) == (o.size, o.mtime_ns, o.ctime_ns, o.inode)
    }
}
impl Eq for Stamp {}

pub fn stamp_of(meta: &fs::Metadata) -> Stamp {
    Stamp {
        size: meta.size(),
        mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        ctime_ns: meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
        inode: meta.ino(),
        device: meta.dev(),
    }
}

/// Stamp of a regular, non-symlink file.
pub fn stamp(path: &Path) -> Result<Stamp> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    ensure!(meta.is_file(), "not a regular file: {}", path.display());
    Ok(stamp_of(&meta))
}

pub fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Streamed SHA-256 of every byte, for files too large to hold in memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Semantic Unison's `SDE/0-preserved` digest: SHA-256 over every byte outside
/// the editable `regions` (sorted, non-overlapping (offset, length) pairs), with
/// the file size and the region geometry bound in. Byte for byte the `preserved`
/// hash of Unison's `SemanticDelta.inspect`:
/// "SDE/0-preserved\0" ‖ u64be size ‖ u64be n ‖ (u64be off ‖ u64be len)* ‖
/// for each gap: u64be start ‖ u64be len ‖ bytes. Regions come from a tiffgraph walk.
pub fn preserved_sha256(bytes: &[u8], regions: &[(u64, u64)]) -> Result<String> {
    let size = bytes.len() as u64;
    let mut h = Sha256::new();
    h.update(b"SDE/0-preserved\0");
    h.update(size.to_be_bytes());
    h.update((regions.len() as u64).to_be_bytes());
    for &(off, len) in regions {
        h.update(off.to_be_bytes());
        h.update(len.to_be_bytes());
    }
    let mut position = 0u64;
    let mut gap = |start: u64, stop: u64| {
        h.update(start.to_be_bytes());
        h.update((stop - start).to_be_bytes());
        h.update(&bytes[start as usize..stop as usize]);
    };
    for &(off, len) in regions {
        let end = off.checked_add(len).filter(|&e| e <= size);
        ensure!(off >= position && end.is_some(), "regions must be sorted, disjoint and inside the file");
        gap(position, off);
        position = end.unwrap();
    }
    gap(position, size);
    Ok(hex(&h.finalize()))
}
