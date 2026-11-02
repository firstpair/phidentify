//! Image-data identity for camera raws, TIFFs and JPEGs that are not DNGs.
//!
//! Two files have equal digests exactly when their stored image bytes are
//! equal, in the same order:
//!
//! - TIFF family (TIFF, BigTIFF, and the TIFF-based raws: NEF, CR2, ARW, PEF,
//!   ORF, RW2 headers and others): the strip or tile payloads of every IFD in
//!   the main chain and its SubIFDs, per IFD in walk order.
//!   Panasonic RW2 stores its raw data from RawDataOffset (tag 280) to the end
//!   of the file without byte counts; that span is taken.
//! - Fujifilm RAF: the CFA section named by the RAF header.
//! - JPEG: the primary image's frame, tables and entropy-coded scans, that is
//!   every segment from SOI to EOI except APPn (EXIF, XMP, ICC, thumbnails)
//!   and COM. Trailing data after EOI is left out.
//!
//! Metadata edits that leave the image bytes in place, such as a capture-time
//! shift written into EXIF, keep the digest. Re-encoding, re-compression or a
//! rewritten preview strip changes it. DNGs are refused: their raw-block
//! identity ([`crate::dng_blocks`]) is the one that ignores the previews
//! Lightroom regenerates. CR3, HEIC and other containers are refused.
use crate::dng_blocks::{slice, type_size, u16_at, u32_at, u64_at};
use anyhow::{Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const DOMAIN: &[u8] = b"PHIDENTIFY-IMAGE-DATA-1\0";

/// SHA-256 of the image data of an in-memory TIFF-family file or JPEG.
pub fn sha256(f: &[u8]) -> Result<String> {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    if f.len() >= 3 && f[..3] == [0xFF, 0xD8, 0xFF] {
        h.update(b"J");
        jpeg(f, &mut h)?;
    } else if f.starts_with(b"FUJIFILMCCD-RAW") {
        // Big-endian header: CFA offset at 100, length at 104.
        ensure!(f.len() >= 108, "RAF header outside file");
        let (off, len) = (u32_at(&f[100..], true) as u64, u32_at(&f[104..], true) as u64);
        ensure!(len > 0, "RAF without CFA data");
        h.update(b"F");
        h.update(len.to_le_bytes());
        h.update(slice(f, off, len).map_err(|_| anyhow::anyhow!("RAF CFA data outside file"))?);
    } else if f.len() >= 4 && matches!(&f[..2], b"II" | b"MM") {
        h.update(b"T");
        tiff(f, &mut h)?;
    } else {
        bail!("not a TIFF-family file or JPEG");
    }
    Ok(crate::file::hex(&h.finalize()))
}

fn tiff(f: &[u8], h: &mut Sha256) -> Result<()> {
    let header = slice(f, 0, 16).or_else(|_| slice(f, 0, 8))?;
    let big = &header[..2] == b"MM";
    // 42 TIFF, 0x4f52/0x5352 Olympus ORF, 0x55 Panasonic RW2, 43 BigTIFF.
    let (large, first) = match u16_at(&header[2..4], big) {
        42 | 0x4f52 | 0x5352 | 0x55 => (false, u32_at(&header[4..8], big) as u64),
        43 if header.len() == 16 && u16_at(&header[4..6], big) == 8 => (true, u64_at(&header[8..16], big)),
        _ => bail!("unsupported TIFF header"),
    };
    h.update([big as u8]);
    let mut pending = vec![first];
    let mut seen = HashSet::new();
    let mut images = 0u64;
    while let Some(off) = pending.pop() {
        if off == 0 || !seen.insert(off) {
            continue;
        }
        ensure!(seen.len() <= 4096, "too many TIFF IFDs");
        let count_len: u64 = if large { 8 } else { 2 };
        let nbytes = slice(f, off, count_len)?;
        let n = if large { u64_at(nbytes, big) } else { u16_at(nbytes, big) as u64 };
        ensure!(n <= 8192, "too many TIFF tags");
        let (entry_len, value_len): (usize, usize) = if large { (20, 8) } else { (12, 4) };
        let entries = slice(f, off + count_len, n * entry_len as u64 + value_len as u64)?;
        let get = |want: u16| -> Result<Option<Vec<u64>>> {
            for i in 0..n as usize {
                let e = &entries[i * entry_len..(i + 1) * entry_len];
                if u16_at(&e[..2], big) != want {
                    continue;
                }
                let kind = u16_at(&e[2..4], big);
                let width = match kind {
                    3 => 2,
                    4 | 13 => 4,
                    16 => 8,
                    _ => bail!("unsupported type {kind} for tag {want}"),
                };
                let Some(unit) = type_size(kind) else { bail!("unknown type {kind}") };
                let count = if large { u64_at(&e[4..12], big) } else { u32_at(&e[4..8], big) as u64 };
                ensure!(count <= 1_000_000, "too many TIFF values");
                let len = count * unit;
                let slot = &e[entry_len - value_len..entry_len];
                let data = if len <= value_len as u64 {
                    &slot[..len as usize]
                } else {
                    slice(f, if large { u64_at(slot, big) } else { u32_at(slot, big) as u64 }, len)?
                };
                return Ok(Some(data.chunks_exact(width).map(|c| match width {
                    2 => u16_at(c, big) as u64,
                    4 => u32_at(c, big) as u64,
                    _ => u64_at(c, big),
                }).collect()));
            }
            Ok(None)
        };
        let has = |want: u16| (0..n as usize).any(|i| u16_at(&entries[i * entry_len..], big) == want);
        ensure!(!has(50706), "DNG: the raw-block identity applies instead");
        let offsets = get(324)?.or(get(273)?);
        let counts = get(325)?.or(get(279)?);
        let sub = get(330)?;
        let tail = &entries[n as usize * entry_len..];
        let next = if large { u64_at(tail, big) } else { u32_at(tail, big) as u64 };
        if next != 0 {
            pending.push(next);
        }
        for pos in sub.unwrap_or_default().into_iter().rev() {
            pending.push(pos);
        }
        let (offsets, counts) = match (offsets, get(280)?) {
            // RW2: raw data from RawDataOffset to the end of the file.
            (None, Some(raw)) if raw.len() == 1 && raw[0] < f.len() as u64 => {
                (Some(raw.clone()), Some(vec![f.len() as u64 - raw[0]]))
            }
            (offsets, _) => (offsets, counts),
        };
        let Some(offsets) = offsets else { continue };
        let Some(counts) = counts.filter(|c| c.len() == offsets.len() && c.iter().all(|&n| n > 0)) else {
            bail!("image data without byte counts");
        };
        images += 1;
        h.update(images.to_le_bytes());
        h.update((counts.len() as u64).to_le_bytes());
        for (&off, &count) in offsets.iter().zip(&counts) {
            h.update(count.to_le_bytes());
            h.update(slice(f, off, count).map_err(|_| anyhow::anyhow!("image data outside file"))?);
        }
    }
    ensure!(images > 0, "no image data in TIFF");
    Ok(())
}

fn jpeg(f: &[u8], h: &mut Sha256) -> Result<()> {
    let mut i = 2;
    let mut scans = 0;
    loop {
        // Fill bytes (0xFF runs) may precede a marker.
        while i < f.len() && f[i] == 0xFF && f.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        ensure!(i + 2 <= f.len() && f[i] == 0xFF, "JPEG ends before EOI");
        let marker = f[i + 1];
        if marker == 0xD9 {
            h.update([0xFF, 0xD9]);
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        ensure!(i + 4 <= f.len(), "JPEG segment outside file");
        let len = u16::from_be_bytes([f[i + 2], f[i + 3]]) as usize;
        ensure!(len >= 2 && i + 2 + len <= f.len(), "JPEG segment outside file");
        let segment = &f[i..i + 2 + len];
        i += 2 + len;
        if (0xE0..=0xEF).contains(&marker) || marker == 0xFE {
            continue;
        }
        h.update(segment);
        if marker == 0xDA {
            scans += 1;
            // Entropy-coded data runs to the next marker other than a stuffed
            // 0xFF00 or a restart marker.
            let start = i;
            while i + 1 < f.len() && !(f[i] == 0xFF && f[i + 1] != 0 && !(0xD0..=0xD7).contains(&f[i + 1])) {
                i += 1;
            }
            ensure!(i + 1 < f.len(), "JPEG scan runs past the end of the file");
            h.update(((i - start) as u64).to_le_bytes());
            h.update(&f[start..i]);
        }
    }
    ensure!(scans > 0, "JPEG without a scan");
    Ok(())
}
