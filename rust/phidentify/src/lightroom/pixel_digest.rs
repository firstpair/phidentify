//! Adobe's NewRawImageDigest from full, pre-linearization decoded DNG samples.
//!
//! The caller supplies an independently validated raw IFD offset. Rawler is used
//! only as a decoder: camera crop, linearization, and rendering are not applied.
//! Restricted to integer, single-plane CFA/LinearRaw, lossless classic TIFF.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use md5::{Digest, Md5};
use rawler::RawImageData;
use rawler::bits::Endian;
use rawler::decoders::plain_image_from_ifd;
use rawler::decompressors::ljpeg::LjpegDecompressor;
use rawler::formats::tiff::{Entry, IFD, Value};
use rawler::rawsource::RawSource;

const MAX_SAMPLES: u64 = 200_000_000;
const MAX_FIELD_BYTES: u64 = 1024 * 1024;
const MAX_IFD_FIELD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const TILE_SIZE: usize = 256;
const LINEARIZATION: u16 = 50712;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDigest {
    pub digest: [u8; 16],
    pub width: u32,
    pub height: u32,
}

/// Decode a validated classic-TIFF raw IFD and compute Adobe's tiled digest.
/// Media is opened read-only. The IFD clone is modified only in memory.
struct Prepared {
    reader: BufReader<File>,
    metadata: std::fs::Metadata,
    ifd: IFD,
    width: u32,
    height: u32,
    linearization_count: Option<usize>,
}

fn prepare(
    path: &Path,
    linearization_count: Option<usize>,
    raw_ifd_offset: u32,
) -> Result<Prepared> {
    let file = File::open(path).with_context(|| format!("Open raw samples: {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Raw input is not a regular file");
    ensure!(
        metadata.len() <= MAX_SOURCE_BYTES,
        "Raw input exceeds 512 MiB snapshot cap"
    );
    let mut reader = BufReader::new(file);
    let endian = check_ifd_bounds(&mut reader, metadata.len(), raw_ifd_offset)?;
    // No recursive sub-IFD parsing: the caller has selected/validated this IFD.
    let mut ifd = IFD::new(&mut reader, raw_ifd_offset, 0, 0, endian, &[])
        .map_err(|error| anyhow::anyhow!("Read raw IFD: {error}"))?;
    let width = scalar(&ifd, 256, None)?;
    let height = scalar(&ifd, 257, None)?;
    ensure!(width > 0 && height > 0, "Zero raw dimensions");
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_SAMPLES,
        "Raw sample allocation exceeds cap"
    );
    ensure!(
        matches!(scalar(&ifd, 262, None)?, 32803 | 34892),
        "Only CFA and single-plane LinearRaw photometric interpretations are validated"
    );
    ensure!(
        scalar(&ifd, 277, Some(1))? == 1,
        "Only single-plane raw images are validated"
    );
    ensure!(
        scalar(&ifd, 284, Some(1))? == 1,
        "Separate-planar TIFF storage is not validated"
    );
    ensure!(
        scalar(&ifd, 339, Some(1))? == 1,
        "Floating/signed raw samples are not supported"
    );
    let compression = scalar(&ifd, 259, Some(1))?;
    ensure!(
        matches!(compression, 1 | 7),
        "Only uncompressed/lossless JPEG raw storage is validated"
    );
    let bits = scalar(&ifd, 258, None)?;
    ensure!(
        matches!(bits, 8 | 10 | 12 | 14 | 16),
        "Unsupported raw bit depth"
    );
    // Rawler's specialized packed 10/12-bit routines process complete groups.
    // Reject a partial group instead of hashing an undecoded trailing sample.
    if compression == 1 && matches!(bits, 10 | 12) {
        let stored_width = if ifd.entries.contains_key(&324) {
            scalar(&ifd, 322, None)?
        } else {
            width
        };
        let group = if bits == 10 { 4 } else { 2 };
        ensure!(
            stored_width.is_multiple_of(group),
            "Partial packed raw sample groups are not validated"
        );
    }
    ensure!(
        scalar(&ifd, 50975, Some(1))? == 1,
        "Row-interleaved DNG is not validated"
    );
    ensure!(
        scalar(&ifd, 52547, Some(1))? == 1,
        "Column-interleaved DNG is not validated"
    );
    validate_storage(&ifd, width, height, metadata.len())?;

    let actual_linearization_count = ifd
        .entries
        .get(&LINEARIZATION)
        .map(|entry| entry.count() as usize);
    ensure!(
        actual_linearization_count == linearization_count,
        "Linearization table count differs from metadata preflight"
    );
    ensure!(
        linearization_count != Some(0),
        "Empty linearization table is invalid"
    );
    // Rawler otherwise linearizes (and may dither) decoded samples. Adobe hashes
    // the original sample indices, so this tag must not reach that operation.
    ifd.entries.remove(&LINEARIZATION);
    // TIFF defaults SamplesPerPixel to one; Rawler requires an explicit entry.
    ifd.entries.entry(277).or_insert_with(|| Entry {
        tag: 277,
        value: Value::Short(vec![1]),
        embedded: None,
    });

    Ok(Prepared {
        reader,
        metadata,
        ifd,
        width,
        height,
        linearization_count,
    })
}

pub fn compute(
    path: &Path,
    linearization_count: Option<usize>,
    raw_ifd_offset: u32,
) -> Result<PixelDigest> {
    Ok(compute_with_proof(path, linearization_count, raw_ifd_offset)?.0)
}

pub fn compute_with_proof(
    path: &Path,
    linearization_count: Option<usize>,
    raw_ifd_offset: u32,
) -> Result<(PixelDigest, proof::InputProof)> {
    let Prepared {
        mut reader,
        metadata,
        ifd,
        width,
        height,
        linearization_count,
    } = prepare(path, linearization_count, raw_ifd_offset)?;
    let compression = scalar(&ifd, 259, Some(1))?;
    // A decoder mmap can SIGBUS when another application truncates the source;
    // catch_unwind cannot catch that signal. Use a bounded owned byte snapshot
    // from the same already-open descriptor, then validate its identity again.
    reader.seek(SeekFrom::Start(0))?;
    let length = usize::try_from(metadata.len())?;
    let mut snapshot = Vec::new();
    snapshot
        .try_reserve_exact(length)
        .context("Allocate bounded raw source snapshot")?;
    snapshot.resize(length, 0);
    reader
        .read_exact(&mut snapshot)
        .context("Read raw source snapshot (source may have changed)")?;
    ensure!(
        same_file_state(&metadata, &reader.get_ref().metadata()?),
        "Source changed while reading raw snapshot"
    );
    let source = RawSource::new_from_shared_vec(Arc::new(snapshot)).with_path(path);
    let input_proof = proof::proof_from_prepared(
        &ifd,
        width,
        height,
        linearization_count,
        metadata.len(),
        &mut std::io::Cursor::new(source.buf()),
    )?;
    if compression == 7 {
        validate_ljpeg_frames(&ifd, &source, width, height)?;
    }
    let decoded = catch_unwind(AssertUnwindSafe(|| plain_image_from_ifd(&ifd, &source)))
        .map_err(|_| anyhow::anyhow!("Rawler panicked while decoding raw samples"))?
        .map_err(|error| anyhow::anyhow!("Decode raw samples: {error}"))?;
    let RawImageData::Integer(pixels) = decoded else {
        bail!("Decoder returned unsupported floating-point samples");
    };
    drop(source);
    let byte_samples = linearization_count.is_some_and(|count| count <= 256);
    let digest = digest_samples(&pixels, width as usize, height as usize, 1, byte_samples)?;
    Ok((
        PixelDigest {
            digest,
            width,
            height,
        },
        input_proof,
    ))
}

fn validate_ljpeg_frames(ifd: &IFD, source: &RawSource, width: u32, height: u32) -> Result<()> {
    let tiled = ifd.entries.contains_key(&324);
    let offsets = array(ifd, if tiled { 324 } else { 273 })?;
    let counts = array(ifd, if tiled { 325 } else { 279 })?;
    let tile_samples = if tiled {
        u64::from(scalar(ifd, 322, None)?) * u64::from(scalar(ifd, 323, None)?)
    } else {
        0
    };
    let rows_per_strip = if tiled {
        0
    } else {
        scalar(ifd, 278, Some(height))?
    };
    for (index, (offset, count)) in offsets.into_iter().zip(counts).enumerate() {
        let bytes = source.subview(offset, count)?;
        // Parsing the lossless JPEG header builds small Huffman tables, but no
        // pixel array. Rawler otherwise allocates from these dimensions before
        // it checks/copies into the already-bounded TIFF output buffer.
        let frame = catch_unwind(AssertUnwindSafe(|| LjpegDecompressor::new(bytes)))
            .map_err(|_| anyhow::anyhow!("Rawler panicked while reading lossless JPEG header"))?
            .map_err(|error| anyhow::anyhow!("Read lossless JPEG header: {error}"))?;
        let expected = if tiled {
            tile_samples
        } else {
            let first_row = index as u64 * u64::from(rows_per_strip);
            let rows = u64::from(height)
                .checked_sub(first_row)
                .context("Strip begins after image bounds")?
                .min(u64::from(rows_per_strip));
            u64::from(width) * rows
        };
        validate_frame_geometry(frame.width(), frame.height(), expected)?;
        ensure!(
            frame.super_h() == 1 && frame.super_v() == 1,
            "Subsampled lossless JPEG raw data is not validated"
        );
    }
    Ok(())
}

fn validate_frame_geometry(width: usize, height: usize, expected: u64) -> Result<()> {
    let samples = width
        .checked_mul(height)
        .context("Lossless JPEG geometry overflow")?;
    ensure!(
        samples as u64 <= MAX_SAMPLES,
        "Lossless JPEG temporary allocation exceeds cap"
    );
    ensure!(
        samples as u64 == expected && expected > 0,
        "Lossless JPEG sample geometry differs from TIFF storage geometry"
    );
    Ok(())
}

fn same_file_state(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn scalar(ifd: &IFD, tag: u16, default: Option<u32>) -> Result<u32> {
    let Some(entry) = ifd.entries.get(&tag) else {
        return default.with_context(|| format!("Missing TIFF tag {tag}"));
    };
    ensure!(entry.count() == 1, "TIFF tag {tag} is not scalar");
    match &entry.value {
        Value::Short(values) => Ok(u32::from(values[0])),
        Value::Long(values) => Ok(values[0]),
        _ => bail!("TIFF tag {tag} has unsupported scalar type"),
    }
}

fn array(ifd: &IFD, tag: u16) -> Result<Vec<u64>> {
    match &ifd
        .entries
        .get(&tag)
        .with_context(|| format!("Missing TIFF array {tag}"))?
        .value
    {
        Value::Short(values) => Ok(values.iter().copied().map(u64::from).collect()),
        Value::Long(values) => Ok(values.iter().copied().map(u64::from).collect()),
        _ => bail!("Unsupported TIFF array type for {tag}"),
    }
}

fn validate_storage(ifd: &IFD, width: u32, height: u32, file_len: u64) -> Result<()> {
    let tiled = ifd.entries.contains_key(&324);
    ensure!(
        tiled != ifd.entries.contains_key(&273),
        "Expected exactly one raw TIFF storage mode"
    );
    let (offset_tag, count_tag, expected) = if tiled {
        let tw = scalar(ifd, 322, None)?;
        let th = scalar(ifd, 323, None)?;
        ensure!(tw > 0 && th > 0, "Zero TIFF tile dimensions");
        let across = u64::from(width).div_ceil(u64::from(tw));
        let down = u64::from(height).div_ceil(u64::from(th));
        let padded_samples = across
            .checked_mul(u64::from(tw))
            .and_then(|value| value.checked_mul(down))
            .and_then(|value| value.checked_mul(u64::from(th)))
            .context("Padded raw tile allocation overflows")?;
        ensure!(
            padded_samples <= MAX_SAMPLES,
            "Padded raw tile allocation exceeds cap"
        );
        (324, 325, across * down)
    } else {
        let rows = scalar(ifd, 278, Some(height))?;
        ensure!(rows > 0, "Zero TIFF rows per strip");
        (273, 279, u64::from(height).div_ceil(u64::from(rows)))
    };
    let offsets = array(ifd, offset_tag)?;
    let counts = array(ifd, count_tag)?;
    ensure!(
        offsets.len() == counts.len() && offsets.len() as u64 == expected,
        "Raw payload range count mismatch"
    );
    let mut ranges = Vec::with_capacity(offsets.len());
    for (offset, count) in offsets.into_iter().zip(counts) {
        ensure!(count > 0, "Empty raw payload");
        let end = offset
            .checked_add(count)
            .context("Raw payload bound overflow")?;
        ensure!(end <= file_len, "Raw payload exceeds EOF");
        ranges.push((offset, end));
    }
    ranges.sort_unstable();
    ensure!(
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "Overlapping raw payload ranges"
    );
    Ok(())
}

// Validate field sizes before asking the decoder's TIFF reader to allocate.
fn check_ifd_bounds<R: Read + Seek>(reader: &mut R, file_len: u64, offset: u32) -> Result<Endian> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header)?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => bail!("Not a TIFF byte order marker"),
    };
    let u16_at = |bytes: &[u8]| match endian {
        Endian::Little => u16::from_le_bytes([bytes[0], bytes[1]]),
        Endian::Big => u16::from_be_bytes([bytes[0], bytes[1]]),
    };
    let u32_at = |bytes: &[u8]| match endian {
        Endian::Little => u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        Endian::Big => u32::from_be_bytes(bytes[..4].try_into().unwrap()),
    };
    ensure!(
        u16_at(&header[2..4]) == 42,
        "Only classic TIFF 42 is supported"
    );
    ensure!(
        offset >= 8 && u64::from(offset) + 2 <= file_len,
        "Invalid raw IFD offset"
    );
    reader.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut count = [0_u8; 2];
    reader.read_exact(&mut count)?;
    let count = u16_at(&count);
    ensure!(
        count > 0 && count <= 4096,
        "Raw IFD entry count exceeds decoder cap"
    );
    ensure!(
        u64::from(offset) + 2 + 12 * u64::from(count) + 4 <= file_len,
        "Raw IFD exceeds EOF"
    );
    let mut total = 0_u64;
    for _ in 0..count {
        let mut entry = [0_u8; 12];
        reader.read_exact(&mut entry)?;
        let unit = match u16_at(&entry[2..4]) {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            other => bail!("Unsupported raw IFD field type {other}"),
        };
        let bytes = u64::from(u32_at(&entry[4..8])) * unit;
        ensure!(
            bytes <= MAX_FIELD_BYTES,
            "Raw IFD field exceeds allocation cap"
        );
        total += bytes;
        ensure!(
            total <= MAX_IFD_FIELD_BYTES,
            "Raw IFD fields exceed aggregate allocation cap"
        );
        if bytes > 4 {
            ensure!(
                u64::from(u32_at(&entry[8..12])) + bytes <= file_len,
                "Raw IFD field exceeds EOF"
            );
        }
    }
    Ok(endian)
}

fn digest_samples(
    pixels: &[u16],
    width: usize,
    height: usize,
    planes: usize,
    byte_samples: bool,
) -> Result<[u8; 16]> {
    let expected = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(planes))
        .context("Pixel dimensions overflow")?;
    ensure!(
        width > 0 && height > 0 && planes > 0 && expected == pixels.len(),
        "Decoded raw dimensions do not match sample count"
    );
    if byte_samples {
        ensure!(
            pixels.iter().all(|value| *value <= 255),
            "Raw samples exceed selected 8-bit digest type"
        );
    }
    let mut outer = Md5::new();
    let mut tile = Vec::with_capacity(TILE_SIZE * TILE_SIZE * planes * 2);
    for top in (0..height).step_by(TILE_SIZE) {
        for left in (0..width).step_by(TILE_SIZE) {
            tile.clear();
            // Rawler's buffer is interleaved; Adobe's tile hash is planar.
            for plane in 0..planes {
                for row in top..height.min(top + TILE_SIZE) {
                    for col in left..width.min(left + TILE_SIZE) {
                        let value = pixels[(row * width + col) * planes + plane];
                        if byte_samples {
                            tile.push(value as u8);
                        } else {
                            tile.extend_from_slice(&value.to_le_bytes());
                        }
                    }
                }
            }
            outer.update(Md5::digest(&tile));
        }
    }
    Ok(outer.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn composed(tiles: &[&[u8]]) -> [u8; 16] {
        let mut outer = Md5::new();
        for tile in tiles {
            outer.update(Md5::digest(tile));
        }
        outer.finalize().into()
    }

    #[test]
    fn little_endian_single_tile() {
        assert_eq!(
            digest_samples(&[0x1234, 0xabcd, 0, 1], 2, 2, 1, false).unwrap(),
            composed(&[&[0x34, 0x12, 0xcd, 0xab, 0, 0, 1, 0]])
        );
    }

    #[test]
    fn tile_edges_are_clipped_and_tile_order_is_row_major() {
        let width = 257;
        let height = 258;
        let mut pixels = vec![0; width * height];
        for row in 0..height {
            for col in 0..width {
                pixels[row * width + col] = match (row < 256, col < 256) {
                    (true, true) => 1,
                    (true, false) => 2,
                    (false, true) => 3,
                    (false, false) => 4,
                };
            }
        }
        assert_eq!(
            digest_samples(&pixels, width, height, 1, true).unwrap(),
            composed(&[&vec![1; 256 * 256], &vec![2; 256], &vec![3; 512], &[4, 4]])
        );
    }

    #[test]
    fn planar_hash_from_interleaved_decoder_samples() {
        assert_eq!(
            digest_samples(&[1, 10, 2, 20, 3, 30, 4, 40], 2, 2, 2, true).unwrap(),
            composed(&[&[1, 2, 3, 4, 10, 20, 30, 40]])
        );
    }

    #[test]
    fn rejects_wrong_sample_count_and_byte_overflow() {
        assert!(digest_samples(&[1], 2, 1, 1, false).is_err());
        assert!(digest_samples(&[256], 1, 1, 1, true).is_err());
        assert!(digest_samples(&[], 0, 0, 1, false).is_err());
    }

    #[test]
    fn selected_ifd_preflight_rejects_oversized_field_before_allocation() {
        let mut bytes = b"II\x2a\0\x08\0\0\0\x01\0".to_vec();
        bytes.extend_from_slice(&256_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&(MAX_FIELD_BYTES as u32 + 1).to_le_bytes());
        bytes.extend_from_slice(&26_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        let error = check_ifd_bounds(&mut Cursor::new(&bytes), bytes.len() as u64, 8).unwrap_err();
        assert!(error.to_string().contains("allocation cap"));
    }

    #[test]
    fn selected_ifd_preflight_rejects_truncation_and_wrong_magic() {
        let mut bytes = b"MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        assert!(check_ifd_bounds(&mut Cursor::new(&bytes), bytes.len() as u64, 8).is_err());
        bytes[3] = 85;
        assert!(
            check_ifd_bounds(&mut Cursor::new(&bytes), bytes.len() as u64, 8)
                .unwrap_err()
                .to_string()
                .contains("TIFF 42")
        );
    }

    #[test]
    fn codec_geometry_is_bounded_and_cannot_leave_uncopied_pixels() {
        assert!(validate_frame_geometry(32768, 32768, 256).is_err());
        assert!(validate_frame_geometry(8, 2, 32).is_err());
        assert!(validate_frame_geometry(8, 4, 32).is_ok());
        assert!(validate_frame_geometry(0, 4, 0).is_err());
    }
}

#[path = "proof.rs"]
pub mod proof;
