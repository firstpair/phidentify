//! Synthetic DNGs: the raw-block identity follows the raw data and ignores
//! metadata; the whole-file SHA-256 follows every byte.
use phidentify::{Options, dng_blocks, identify};
use std::path::PathBuf;

/// Minimal little-endian classic-TIFF DNG: one IFD with DNGVersion, a CFA raw
/// strip and an out-of-line XMP packet.
fn dng(raw: &[u8], xmp: &[u8], model_pad: u8) -> Vec<u8> {
    // (tag, type, count, value-or-offset placeholder)
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (254, 4, 1, 0u32.to_le_bytes().to_vec()),
        (256, 4, 1, 4u32.to_le_bytes().to_vec()),
        (257, 4, 1, (raw.len() as u32 / 4).to_le_bytes().to_vec()),
        (258, 3, 1, 8u16.to_le_bytes().to_vec()),
        (259, 3, 1, 1u16.to_le_bytes().to_vec()),
        (262, 3, 1, 32803u16.to_le_bytes().to_vec()),
        (273, 4, 1, vec![]), // strip offset, patched below
        (277, 3, 1, 1u16.to_le_bytes().to_vec()),
        (279, 4, 1, (raw.len() as u32).to_le_bytes().to_vec()),
        (700, 1, xmp.len() as u32, vec![]), // XMP offset, patched below
        (50706, 1, 4, vec![1, 4, 0, 0]),
    ];
    let ifd_len = 2 + tags.len() * 12 + 4;
    let xmp_off = 8 + ifd_len + model_pad as usize; // padding moves data without changing it
    let raw_off = xmp_off + xmp.len();
    for t in tags.iter_mut() {
        match t.0 {
            273 => t.3 = (raw_off as u32).to_le_bytes().to_vec(),
            700 => t.3 = (xmp_off as u32).to_le_bytes().to_vec(),
            _ => {}
        }
    }
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    for (code, kind, count, value) in &tags {
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        let mut slot = value.clone();
        slot.resize(4, 0);
        out.extend_from_slice(&slot);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.resize(xmp_off, 0);
    out.extend_from_slice(xmp);
    out.extend_from_slice(raw);
    out
}

fn blocks(bytes: &[u8]) -> String {
    dng_blocks::raw_blocks_sha256(bytes, &dng_blocks::parse(bytes).unwrap()).unwrap()
}

#[test]
fn raw_block_identity_ignores_xmp_and_placement_but_not_raw_bytes() {
    let raw: Vec<u8> = (0..64).collect();
    let base = dng(&raw, b"<x:xmpmeta>rating 1</x:xmpmeta>", 0);
    let edited = dng(&raw, b"<x:xmpmeta>rating 5, keyword</x:xmpmeta>", 3);
    let mut changed_raw = raw.clone();
    changed_raw[10] ^= 1;
    let changed = dng(&changed_raw, b"<x:xmpmeta>rating 1</x:xmpmeta>", 0);
    assert_eq!(blocks(&base), blocks(&edited));
    assert_ne!(blocks(&base), blocks(&changed));
    assert_eq!(dng_blocks::parse(&base).unwrap().xmp.len(), 1);
}

#[test]
fn non_dng_tiff_and_truncated_raw_are_errors_not_matches() {
    let raw: Vec<u8> = (0..64).collect();
    let mut no_version = dng(&raw, b"<x/>", 0);
    // Rename tag 50706 to an unselected private tag: a TIFF that is not a DNG.
    let pos = no_version.windows(2).position(|w| w == 50706u16.to_le_bytes()).unwrap();
    no_version[pos..pos + 2].copy_from_slice(&50000u16.to_le_bytes());
    assert!(dng_blocks::parse(&no_version).is_err());
    let full = dng(&raw, b"<x/>", 0);
    let truncated = &full[..full.len() - 1];
    let parsed = dng_blocks::parse(truncated).unwrap();
    assert!(dng_blocks::raw_blocks_sha256(truncated, &parsed).is_err());
    assert!(dng_blocks::parse(b"\xff\xd8\xff\xe0 not a tiff").is_err());
}

#[test]
fn identify_reports_each_identity_separately() {
    let dir = std::env::temp_dir().join(format!("phidentify-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw: Vec<u8> = (0..64).collect();
    let a: PathBuf = dir.join("a.dng");
    let b: PathBuf = dir.join("b.dng");
    let j: PathBuf = dir.join("c.jpg");
    std::fs::write(&a, dng(&raw, b"<x:xmpmeta>1</x:xmpmeta>", 0)).unwrap();
    std::fs::write(&b, dng(&raw, b"<x:xmpmeta>22</x:xmpmeta>", 0)).unwrap();
    std::fs::write(&j, b"\xff\xd8\xff\xe0 synthetic").unwrap();
    let opts = Options { dng_blocks: true, lightroom: false, metadata: false };
    let (ia, ib, ij) = (identify(&a, opts).unwrap(), identify(&b, opts).unwrap(), identify(&j, opts).unwrap());
    assert_ne!(ia.sha256, ib.sha256);
    assert_eq!(ia.dng_raw_blocks_sha256, ib.dng_raw_blocks_sha256);
    assert!(matches!(ia.dng_raw_blocks_sha256, Some(Ok(_))));
    assert_eq!(ij.dng_raw_blocks_sha256, None, "a non-TIFF file is not attempted");
    assert_eq!(ia.lightroom_digest, None);
    assert_eq!(ia.sha256, phidentify::file::sha256_file(&a).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}
