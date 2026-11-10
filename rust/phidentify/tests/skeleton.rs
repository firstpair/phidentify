//! Skeletons and block marks on synthetic files: a small classic TIFF with an
//! XMP packet, a strip and a large private tag, a JPEG, and a sidecar.
use phidentify::skeleton::{self, Skeleton};
use phidentify::{Options, identify};
use std::path::PathBuf;

/// A little-endian TIFF: ImageDescription, a 2,048-byte strip, an XMP packet
/// (tag 700), a 600-byte private tag (50740) and DNGVersion. `fill` seeds the
/// strip, `private` the private tag.
fn tiff(xmp: &str, fill: u8, private: u8) -> Vec<u8> {
    let strip: Vec<u8> = (0..2048u32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(fill)).collect();
    let blob: Vec<u8> = (0..600u32).map(|i| (i as u8).wrapping_mul(3).wrapping_add(private)).collect();
    let entries: u32 = 9;
    let ifd = 8u32;
    let xmp_at = ifd + 2 + entries * 12 + 4;
    let blob_at = xmp_at + xmp.len() as u32 + (xmp.len() as u32 & 1);
    let strip_at = blob_at + blob.len() as u32;
    let mut out = vec![b'I', b'I', 42, 0];
    out.extend(ifd.to_le_bytes());
    out.extend((entries as u16).to_le_bytes());
    let mut tag = |t: u16, ty: u16, n: u32, v: u32| {
        out.extend(t.to_le_bytes());
        out.extend(ty.to_le_bytes());
        out.extend(n.to_le_bytes());
        out.extend(v.to_le_bytes());
    };
    tag(256, 4, 1, 32);
    tag(257, 4, 1, 32);
    tag(258, 3, 1, 16);
    tag(259, 3, 1, 1);
    tag(273, 4, 1, strip_at);
    tag(279, 4, 1, strip.len() as u32);
    tag(700, 1, xmp.len() as u32, xmp_at);
    tag(50706, 1, 4, u32::from_le_bytes([1, 4, 0, 0]));
    tag(50740, 1, blob.len() as u32, blob_at);
    out.extend(0u32.to_le_bytes());
    out.extend(xmp.as_bytes());
    if xmp.len() & 1 == 1 {
        out.push(0);
    }
    out.extend(&blob);
    out.extend(&strip);
    out
}

/// A JPEG: JFIF, an Exif segment holding `note`, a 700-byte ICC-like APP2, a
/// quantization table and a scan of `scan` bytes.
fn jpeg(note: &str, scan: u8) -> Vec<u8> {
    let mut out = vec![0xff, 0xd8];
    let mut seg = |marker: u8, body: &[u8]| {
        out.extend([0xff, marker]);
        out.extend(((body.len() + 2) as u16).to_be_bytes());
        out.extend(body);
    };
    seg(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    seg(0xe1, format!("Exif\0\0{note}").as_bytes());
    seg(0xe2, &(0..700u32).map(|i| i as u8).collect::<Vec<_>>());
    seg(0xdb, &[0u8; 65]);
    out.extend([0xff, 0xda, 0, 8, 1, 1, 0, 0, 63, 0]);
    out.extend((0..3000u32).map(|i| ((i as u8) ^ scan) & 0x7f));
    out.extend([0xff, 0xd9]);
    out
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("phidentify-skeleton-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn restored(old: &Skeleton, edited: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let sha = skeleton::rebuild_from(old, &mut skeleton::Bytes(edited), &mut out)?;
    assert_eq!(sha, phidentify::file::sha256_bytes(&out));
    Ok(out)
}

#[test]
fn a_tiff_is_rebuilt_from_its_edited_version() {
    let old = tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 1);
    let edited = tiff(&format!("<x:xmpmeta>rating 5, exposure +0.35{}</x:xmpmeta>", " ".repeat(301)), 1, 1);
    let (a, b) = (skeleton::build_bytes(&old).unwrap(), skeleton::build_bytes(&edited).unwrap());
    assert_eq!(a.format, "tiff");
    let roles: Vec<&str> = a.blocks.iter().map(|b| b.role.as_str()).collect();
    assert_eq!(roles, ["0/tag50740", "0/strips"]);
    assert_eq!(a.blocks.iter().map(|b| b.len).collect::<Vec<_>>(), [600, 2048]);
    assert_ne!(a.blocks[0].at, b.blocks[0].at, "the edit moved the blocks");
    assert_eq!(a.bytes.len() + 2648, old.len());
    assert_eq!(restored(&a, &edited).unwrap(), old);
    assert_eq!(restored(&b, &old).unwrap(), edited, "and the other way");
    // The identity ignores where the blocks sit.
    assert_eq!(skeleton::block_marks(&a).unwrap(), skeleton::block_marks(&b).unwrap());
}

#[test]
fn a_changed_block_is_another_identity_and_does_not_rebuild() {
    let old = tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 1);
    let a = skeleton::build_bytes(&old).unwrap();
    for other in [tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 2, 1), tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 2)] {
        let b = skeleton::build_bytes(&other).unwrap();
        assert_ne!(skeleton::block_marks(&a).unwrap(), skeleton::block_marks(&b).unwrap());
        let err = restored(&a, &other).unwrap_err();
        assert!(err.contains("is not in the edited file"), "{err}");
    }
}

#[test]
fn a_change_inside_a_block_keeps_the_marks_and_fails_the_sha256() {
    // A strip longer than two marks, changed in its middle only.
    let mut old = tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 1);
    old.extend(vec![9u8; 10_000]);
    let n = old.len() as u32;
    // Point the strip at the appended run.
    let at = (n - 10_000).to_le_bytes();
    let pos = old.windows(4).position(|w| w == [0x11, 0x01, 4, 0]).unwrap();
    old[pos + 8..pos + 12].copy_from_slice(&at);
    let pos = old.windows(4).position(|w| w == [0x17, 0x01, 4, 0]).unwrap();
    old[pos + 8..pos + 12].copy_from_slice(&10_000u32.to_le_bytes());
    let mut edited = old.clone();
    let middle = edited.len() - 5_000;
    edited[middle] ^= 1;
    let (a, b) = (skeleton::build_bytes(&old).unwrap(), skeleton::build_bytes(&edited).unwrap());
    assert_eq!(skeleton::block_marks(&a).unwrap(), skeleton::block_marks(&b).unwrap(), "a sampled identity");
    let out = restored(&a, &edited).unwrap();
    assert_ne!(phidentify::file::sha256_bytes(&out), phidentify::file::sha256_bytes(&old), "caught by the whole-file SHA-256");
}

#[test]
fn a_jpeg_is_rebuilt_from_its_edited_version() {
    let old = jpeg("caption: none", 0);
    let edited = jpeg("caption: a much longer caption written by an editor, with keywords", 0);
    let (a, b) = (skeleton::build_bytes(&old).unwrap(), skeleton::build_bytes(&edited).unwrap());
    assert_eq!(a.format, "jpeg");
    assert_eq!(a.blocks.iter().map(|b| b.role.as_str()).collect::<Vec<_>>(), ["sege2", "scan"]);
    assert_eq!(restored(&a, &edited).unwrap(), old);
    assert_eq!(skeleton::block_marks(&a).unwrap(), skeleton::block_marks(&b).unwrap());
    let other = skeleton::build_bytes(&jpeg("caption: none", 1)).unwrap();
    assert_ne!(skeleton::block_marks(&a).unwrap(), skeleton::block_marks(&other).unwrap());
}

#[test]
fn a_small_file_of_another_format_is_its_own_skeleton() {
    let sidecar = b"<x:xmpmeta>rating 3</x:xmpmeta>".to_vec();
    let s = skeleton::build_bytes(&sidecar).unwrap();
    assert_eq!((s.format.as_str(), s.blocks.len()), ("whole", 0));
    assert_eq!(restored(&s, b"anything").unwrap(), sidecar);
    assert_eq!(skeleton::block_marks(&s).unwrap_err(), skeleton::NO_WALKER);
    let large = vec![7u8; (skeleton::WHOLE + 1) as usize];
    assert_eq!(skeleton::build_bytes(&large).unwrap_err(), skeleton::NO_WALKER);
}

#[test]
fn a_record_and_its_packed_bytes_give_the_skeleton_back() {
    let old = tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 1);
    let s = skeleton::build_bytes(&old).unwrap();
    let sha = phidentify::file::sha256_bytes(&old);
    let (record, packed) = (s.record(&sha), s.pack().unwrap());
    assert!(record.starts_with(&format!("{{\"sha256\": \"{sha}\", \"size\": {}, \"format\": \"tiff\", \"blocks\": [[", old.len())), "{record}");
    assert!(record.ends_with("\"skeleton_format\": \"zstd\"}"), "{record}");
    assert_eq!(Skeleton::from_record(&record, &packed).unwrap(), s);
    let mut wrong = s.clone();
    wrong.bytes[0] ^= 1;
    assert!(Skeleton::from_record(&record, &wrong.pack().unwrap()).is_err(), "bytes that are not the record's");
    assert!(Skeleton::from_record(&record.replace("\"size\": ", "\"size\": 1"), &packed).is_ok_and(|x| x.size != s.size));
    assert!(Skeleton::from_record(&record.replacen("[[", "[[99999, 1, \"\", \"\"], [", 1), &packed).is_err(), "blocks out of order");
}

#[test]
fn a_file_on_disk_and_in_memory_cut_alike_and_identify_reports_both() {
    let d = dir("identify");
    let bytes = tiff("<x:xmpmeta>rating 0</x:xmpmeta>", 1, 1);
    let path = d.join("a.dng");
    std::fs::write(&path, &bytes).unwrap();
    let on_disk = skeleton::build(&path).unwrap();
    assert_eq!(on_disk, skeleton::build_bytes(&bytes).unwrap());
    let opts = Options { block_marks: true, skeleton: true, ..Options::default() };
    let id = identify(&path, opts).unwrap();
    assert_eq!(id.block_marks_sha256, Some(skeleton::block_marks(&on_disk)));
    assert_eq!(id.skeleton, Some(Ok(on_disk.clone())));
    let id = identify(&path, Options::default()).unwrap();
    assert!(id.block_marks_sha256.is_none() && id.skeleton.is_none(), "not attempted unless asked for");
    let mut out = Vec::new();
    let edited = d.join("b.dng");
    std::fs::write(&edited, tiff("<x:xmpmeta>rating 4 and more</x:xmpmeta>", 1, 1)).unwrap();
    assert_eq!(skeleton::rebuild(&on_disk, &edited, &mut out).unwrap(), id.sha256);
    assert_eq!(out, bytes);
    let _ = std::fs::remove_dir_all(&d);
}
