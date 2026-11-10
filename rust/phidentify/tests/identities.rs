//! Synthetic DNGs: the raw-block identity follows the raw data and ignores
//! metadata; the whole-file SHA-256 follows every byte.
use phidentify::{Options, dng_blocks, identify};
use std::path::PathBuf;

/// Minimal little-endian classic-TIFF DNG: one IFD with DNGVersion, a CFA raw
/// strip and an out-of-line XMP packet.
fn dng(raw: &[u8], xmp: &[u8], model_pad: u8) -> Vec<u8> {
    tiff(raw, xmp, model_pad, true)
}

/// The same file, with or without DNGVersion (a TIFF-based camera raw).
fn tiff(raw: &[u8], xmp: &[u8], model_pad: u8, dng_version: bool) -> Vec<u8> {
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
    if !dng_version {
        tags.pop();
    }
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
    let opts = Options { dng_blocks: true, lightroom: false, metadata: false, preserved_regions: None, dng_semantic: false, image_data: true, block_marks: false, skeleton: false };
    let (ia, ib, ij) = (identify(&a, opts.clone()).unwrap(), identify(&b, opts.clone()).unwrap(), identify(&j, opts).unwrap());
    assert_ne!(ia.sha256, ib.sha256);
    assert_eq!(ia.dng_raw_blocks_sha256, ib.dng_raw_blocks_sha256);
    assert!(matches!(ia.dng_raw_blocks_sha256, Some(Ok(_))));
    assert_eq!(ij.dng_raw_blocks_sha256, None, "a non-TIFF file is not attempted");
    assert_eq!(ia.lightroom_digest, None);
    assert_eq!(ia.sha256, phidentify::file::sha256_file(&a).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn preserved_digest_follows_unisons_formula_and_ignores_region_bytes() {
    use sha2::{Digest, Sha256};
    let a = b"HEADER<xmp one>TRAILER".to_vec();
    let mut b = a.clone();
    b[7..14].copy_from_slice(b"XMP TWO");
    let region = [(6u64, 9u64)];
    let pa = phidentify::file::preserved_sha256(&a, &region).unwrap();
    assert_eq!(pa, phidentify::file::preserved_sha256(&b, &region).unwrap());
    let mut h = Sha256::new();
    h.update(b"SDE/0-preserved\0");
    h.update(22u64.to_be_bytes());
    h.update(1u64.to_be_bytes());
    h.update(6u64.to_be_bytes());
    h.update(9u64.to_be_bytes());
    h.update(0u64.to_be_bytes());
    h.update(6u64.to_be_bytes());
    h.update(b"HEADER");
    h.update(15u64.to_be_bytes());
    h.update(7u64.to_be_bytes());
    h.update(b"TRAILER");
    assert_eq!(pa, phidentify::file::hex(&h.finalize()));
    let mut c = a.clone();
    c[0] = b'h';
    assert_ne!(pa, phidentify::file::preserved_sha256(&c, &region).unwrap());
    assert!(phidentify::file::preserved_sha256(&a, &[(20, 5)]).is_err());
    assert!(phidentify::file::preserved_sha256(&a, &[(6, 4), (8, 2)]).is_err());
}

/// A baseline JPEG skeleton: APP segments and COM around fixed tables and a scan.
fn jpeg(app1: &[u8], comment: &[u8], scan: &[u8], trailer: &[u8]) -> Vec<u8> {
    let seg = |marker: u8, body: &[u8]| {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(body);
        v
    };
    let mut out = vec![0xFF, 0xD8];
    out.extend(seg(0xE1, app1));
    out.extend(seg(0xDB, &[0; 65]));
    out.extend(seg(0xC0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]));
    out.extend(seg(0xFE, comment));
    out.extend(seg(0xC4, &[0; 20]));
    out.extend(seg(0xDA, &[1, 1, 0, 0, 63, 0]));
    out.extend_from_slice(scan);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out.extend_from_slice(trailer);
    out
}

#[test]
fn image_data_ignores_metadata_of_raws_and_jpegs_but_not_image_bytes() {
    use phidentify::image_data::sha256;
    let raw: Vec<u8> = (0..64).collect();
    let nef = tiff(&raw, b"<x:xmpmeta>2024:10:14 17:36</x:xmpmeta>", 0, false);
    let shifted = tiff(&raw, b"<x:xmpmeta>2024:10:14 17:42:03</x:xmpmeta>", 5, false);
    let mut other_raw = raw.clone();
    other_raw[9] ^= 1;
    assert_ne!(nef, shifted);
    assert_eq!(sha256(&nef).unwrap(), sha256(&shifted).unwrap());
    assert_ne!(sha256(&nef).unwrap(), sha256(&tiff(&other_raw, b"x", 0, false)).unwrap());
    assert!(sha256(&dng(&raw, b"x", 0)).unwrap_err().to_string().starts_with("DNG"));

    let scan = [1, 2, 0xFF, 0x00, 3, 0xFF, 0xD0, 4];
    let a = jpeg(b"Exif\0\0 DateTimeOriginal 17:36", b"one", &scan, b"");
    let b = jpeg(b"Exif\0\0 DateTimeOriginal 17:42:03 longer", b"two", &scan, b"MPF trailer");
    let mut changed = scan;
    changed[0] = 9;
    assert_eq!(sha256(&a).unwrap(), sha256(&b).unwrap());
    assert_ne!(sha256(&a).unwrap(), sha256(&jpeg(b"Exif", b"one", &changed, b"")).unwrap());
    assert!(sha256(&a[..a.len() - 2]).is_err(), "a JPEG cut before EOI is refused");
    assert!(sha256(b"not an image").is_err());
}

#[test]
fn image_data_of_rw2_and_raf_spans_their_raw_data() {
    use phidentify::image_data::sha256;
    // RW2: IFD0 with only RawDataOffset (280); raw data runs to the end of the file.
    let rw2 = |meta: &[u8], raw: &[u8]| {
        let mut f = b"IIU\0".to_vec();
        f.extend_from_slice(&8u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        let raw_off = 8 + 2 + 12 + 4 + meta.len();
        f.extend_from_slice(&280u16.to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&(raw_off as u32).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(meta);
        f.extend_from_slice(raw);
        f
    };
    assert_eq!(sha256(&rw2(b"a", b"RAWDATA")).unwrap(), sha256(&rw2(b"longer meta", b"RAWDATA")).unwrap());
    assert_ne!(sha256(&rw2(b"a", b"RAWDATA")).unwrap(), sha256(&rw2(b"a", b"RAWDATB")).unwrap());
    // RAF: header with the CFA offset at 100 and length at 104 (big-endian).
    let raf = |jpeg: &[u8], cfa: &[u8]| {
        let mut f = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        f.resize(108, 0);
        f.extend_from_slice(jpeg);
        let off = f.len() as u32;
        f[100..104].copy_from_slice(&off.to_be_bytes());
        f[104..108].copy_from_slice(&(cfa.len() as u32).to_be_bytes());
        f.extend_from_slice(cfa);
        f
    };
    assert_eq!(sha256(&raf(b"exif one", b"CFA")).unwrap(), sha256(&raf(b"exif two, shifted", b"CFA")).unwrap());
    assert_ne!(sha256(&raf(b"x", b"CFA")).unwrap(), sha256(&raf(b"x", b"CFB")).unwrap());
}

/// The metadata identity is attempted only for extensions the engine reads: the
/// same DNG bytes named as a Capture One `.cop` cache proxy are not attempted
/// (None, not a failure), and an `.ini` is not either.
#[cfg(feature = "metadata")]
#[test]
fn metadata_only_for_extensions_the_engine_reads() {
    let dir = std::env::temp_dir().join(format!("phidentify-test-md-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = dng(&(0..64).collect::<Vec<u8>>(), b"<x:xmpmeta>1</x:xmpmeta>", 0);
    let (d, c, i) = (dir.join("a.DNG"), dir.join("a.DNG.cop"), dir.join("settings.ini"));
    std::fs::write(&d, &bytes).unwrap();
    std::fs::write(&c, &bytes).unwrap();
    std::fs::write(&i, b"[x]\ny=1\n").unwrap();
    let opts = Options { dng_blocks: false, lightroom: false, metadata: true, preserved_regions: None, dng_semantic: false, image_data: false, block_marks: false, skeleton: false };
    let id = |p: &PathBuf| identify(p, opts.clone()).unwrap();
    assert!(id(&d).metadata_digest.is_some(), "a .DNG is attempted");
    for p in [&c, &i] {
        let r = id(p);
        assert_eq!((r.metadata_digest, r.metadata_json), (None, None), "{}", p.display());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
