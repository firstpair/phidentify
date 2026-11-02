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
