//! DNG encoded raw-block identity, after dng-compare's `dng-subset` scanner.
//!
//! Two files have equal digests exactly when dng-subset's block comparison
//! finds them equal: same byte order, same number of primary raw images, and
//! for each, the same encoding-tag signature, block counts and block bytes.
//! Metadata, XMP, previews and block placement do not enter it. It is not a
//! decoded-sample identity, a rendering identity or Lightroom's fingerprint.
use anyhow::{Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const DOMAIN: &[u8] = b"PHIDENTIFY-DNG-ENCODED-RAW-BLOCKS-1\0";
/// Encoding tags of a raw IFD (dng-subset's ENCODING list).
const ENCODING: &[u16] = &[256, 257, 258, 259, 262, 266, 277, 278, 284, 317, 322, 323, 338, 339, 347, 512,
    515, 517, 518, 519, 520, 521, 529, 530, 531, 532, 50871, 50974, 52543, 52544];

pub(crate) fn u16_at(b: &[u8], big: bool) -> u16 {
    let a = [b[0], b[1]];
    if big { u16::from_be_bytes(a) } else { u16::from_le_bytes(a) }
}
pub(crate) fn u32_at(b: &[u8], big: bool) -> u32 {
    let a = b[0..4].try_into().unwrap();
    if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) }
}
pub(crate) fn u64_at(b: &[u8], big: bool) -> u64 {
    let a = b[0..8].try_into().unwrap();
    if big { u64::from_be_bytes(a) } else { u64::from_le_bytes(a) }
}
pub(crate) fn slice(f: &[u8], off: u64, len: u64) -> Result<&[u8]> {
    let end = off.checked_add(len).filter(|&e| e <= f.len() as u64);
    let Some(end) = end else { bail!("TIFF offset outside file") };
    Ok(&f[off as usize..end as usize])
}
pub(crate) fn type_size(kind: u16) -> Option<u64> {
    match kind {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 | 16 | 17 | 18 => Some(8),
        _ => None,
    }
}

struct Tag<'a> {
    kind: u16,
    count: u64,
    data: &'a [u8],
}
impl Tag<'_> {
    fn nums(&self, big: bool) -> Result<Vec<u64>> {
        let width = match self.kind {
            1 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 13 => 4,
            16..=18 => 8,
            _ => bail!("Unsupported integer TIFF tag type"),
        };
        ensure!(self.count <= 1_000_000, "Too many TIFF integer values");
        Ok(self
            .data
            .chunks_exact(width)
            .map(|c| match width {
                1 => c[0] as u64,
                2 => u16_at(c, big) as u64,
                4 => u32_at(c, big) as u64,
                _ => u64_at(c, big),
            })
            .collect())
    }
}

/// One primary raw image: its encoding signature and block extents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImage {
    pub signature: Vec<u8>,
    pub offsets: Vec<u64>,
    pub counts: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct Dng {
    pub big_endian: bool,
    pub raw: Vec<RawImage>,
    /// Every TIFF tag 700 (XMP) packet, as (offset, length) when stored out of line.
    pub xmp: Vec<(u64, u64)>,
}

/// Parse the TIFF/BigTIFF graph of a DNG held in memory. Fails for non-DNG
/// files and for DNGs without a primary raw image.
pub fn parse(f: &[u8]) -> Result<Dng> {
    let header = slice(f, 0, 16)?;
    let big = match &header[..2] {
        b"MM" => true,
        b"II" => false,
        _ => bail!("Not a TIFF file"),
    };
    let (large, first) = match u16_at(&header[2..4], big) {
        42 => (false, u32_at(&header[4..8], big) as u64),
        43 if u16_at(&header[4..6], big) == 8 && u16_at(&header[6..8], big) == 0 => (true, u64_at(&header[8..16], big)),
        _ => bail!("Unsupported TIFF header"),
    };
    let mut pending = vec![first];
    let mut seen = HashSet::new();
    let (mut raw, mut xmp) = (Vec::new(), Vec::new());
    let mut has_dng_version = false;
    while let Some(off) = pending.pop() {
        if off == 0 || !seen.insert(off) {
            continue;
        }
        ensure!(seen.len() <= 4096, "Too many TIFF IFDs");
        let count_len: u64 = if large { 8 } else { 2 };
        let nbytes = slice(f, off, count_len)?;
        let n = if large { u64_at(nbytes, big) } else { u16_at(nbytes, big) as u64 };
        ensure!(n <= 8192, "Too many TIFF tags");
        let (entry_len, value_len): (usize, usize) = if large { (20, 8) } else { (12, 4) };
        let entries = slice(f, off + count_len, n * entry_len as u64 + value_len as u64)?;
        let mut tags: HashMap<u16, Tag> = HashMap::new();
        for i in 0..n as usize {
            let e = &entries[i * entry_len..(i + 1) * entry_len];
            let code = u16_at(&e[..2], big);
            if code == 50706 {
                has_dng_version = true;
            }
            if !ENCODING.contains(&code) && ![254, 273, 279, 324, 325, 330, 700, 50706].contains(&code) {
                continue;
            }
            let kind = u16_at(&e[2..4], big);
            let Some(unit) = type_size(kind) else { bail!("Unknown type {kind} for selected tag") };
            let count = if large { u64_at(&e[4..12], big) } else { u32_at(&e[4..8], big) as u64 };
            let Some(len) = count.checked_mul(unit) else { bail!("TIFF tag overflow") };
            ensure!(len <= 16 * 1024 * 1024, "TIFF tag too large");
            let slot = &e[entry_len - value_len..entry_len];
            let data = if len <= value_len as u64 {
                &slot[..len as usize]
            } else {
                let pos = if large { u64_at(slot, big) } else { u32_at(slot, big) as u64 };
                if code == 700 {
                    xmp.push((pos, len));
                }
                slice(f, pos, len)?
            };
            tags.insert(code, Tag { kind, count, data });
        }
        let tail = &entries[n as usize * entry_len..];
        let next = if large { u64_at(tail, big) } else { u32_at(tail, big) as u64 };
        if next != 0 {
            pending.push(next);
        }
        if let Some(sub) = tags.get(&330) {
            for pos in sub.nums(big)?.into_iter().rev() {
                pending.push(pos);
            }
        }
        let first_num = |code: u16| -> Result<Option<u64>> {
            Ok(tags.get(&code).map(|t| t.nums(big)).transpose()?.and_then(|v| v.first().copied()))
        };
        let subtype = first_num(254)?.unwrap_or(0);
        let photo = first_num(262)?;
        if subtype == 0 && photo.is_some_and(|p| [0, 1, 2, 32803, 34892].contains(&p)) {
            let Some(offsets) = tags.get(&324).or_else(|| tags.get(&273)) else { bail!("Raw IFD missing offsets") };
            let Some(counts) = tags.get(&325).or_else(|| tags.get(&279)) else { bail!("Raw IFD missing byte counts") };
            let (offsets, counts) = (offsets.nums(big)?, counts.nums(big)?);
            ensure!(!offsets.is_empty() && offsets.len() == counts.len(), "Invalid raw block directory");
            let mut signature = vec![big as u8];
            for code in ENCODING {
                if let Some(tag) = tags.get(code) {
                    signature.extend_from_slice(&code.to_le_bytes());
                    signature.extend_from_slice(&tag.kind.to_le_bytes());
                    signature.extend_from_slice(&tag.count.to_le_bytes());
                    signature.extend_from_slice(&(tag.data.len() as u64).to_le_bytes());
                    signature.extend_from_slice(tag.data);
                }
            }
            raw.push(RawImage { signature, offsets, counts });
        }
    }
    ensure!(has_dng_version && !raw.is_empty(), "No DNG raw image found");
    Ok(Dng { big_endian: big, raw, xmp })
}

/// SHA-256 over the encoded raw-block identity of a parsed DNG.
pub fn raw_blocks_sha256(f: &[u8], dng: &Dng) -> Result<String> {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update([dng.big_endian as u8]);
    h.update((dng.raw.len() as u64).to_le_bytes());
    for image in &dng.raw {
        h.update((image.signature.len() as u64).to_le_bytes());
        h.update(&image.signature);
        h.update((image.counts.len() as u64).to_le_bytes());
        for (&off, &count) in image.offsets.iter().zip(&image.counts) {
            ensure!(count > 0, "Raw block outside file");
            h.update(count.to_le_bytes());
            h.update(slice(f, off, count).map_err(|_| anyhow::anyhow!("Raw block outside file"))?);
        }
    }
    Ok(crate::file::hex(&h.finalize()))
}
