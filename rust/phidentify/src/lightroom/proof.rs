//! Experimental strong equality proof for the exact supported raw decoder input.
//! This is NOT Lightroom's fingerprint and does not trust any embedded digest.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const CONTEXT_DOMAIN: &[u8] = b"LRCAT-RAWLER-0.8.0-V5-INPUT-CONTEXT-1\0";
const PAYLOAD_DOMAIN: &[u8] = b"LRCAT-RAW-PAYLOAD-ORDERED-FRAMES-1\0";
const PROOF_DOMAIN: &[u8] = b"LRCAT-RAW-DECODE-EQUIVALENCE-1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecodeContext {
    pub width: u32,
    pub height: u32,
    pub little_endian: bool,
    pub photometric: u32,
    pub samples_per_pixel: u32,
    pub planar_configuration: u32,
    pub sample_format: u32,
    pub compression: u32,
    pub bits_per_sample: u32,
    pub row_interleave: u32,
    pub column_interleave: u32,
    pub linearization_count: Option<usize>,
    pub adobe_digest_uses_byte_samples: bool,
    pub tiled: bool,
    pub rows_per_strip: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    // Rawler's packed-strip implementation can take its continuous-data branch.
    pub strip_payloads_physically_contiguous: bool,
    pub payload_count: usize,
}

impl DecodeContext {
    fn canonical_bytes(&self) -> Vec<u8> {
        let words = [
            self.width as u64,
            self.height as u64,
            self.little_endian as u64,
            self.photometric as u64,
            self.samples_per_pixel as u64,
            self.planar_configuration as u64,
            self.sample_format as u64,
            self.compression as u64,
            self.bits_per_sample as u64,
            self.row_interleave as u64,
            self.column_interleave as u64,
            self.linearization_count.is_some() as u64,
            self.linearization_count.unwrap_or(0) as u64,
            self.adobe_digest_uses_byte_samples as u64,
            self.tiled as u64,
            self.rows_per_strip as u64,
            self.tile_width as u64,
            self.tile_height as u64,
            self.strip_payloads_physically_contiguous as u64,
            self.payload_count as u64,
        ];
        words.into_iter().flat_map(u64::to_le_bytes).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputProof {
    pub schema: String,
    pub context: DecodeContext,
    pub context_sha256: String,
    pub framed_payload_sha256: String,
    pub combined_sha256: String,
    pub payload_bytes: u64,
    pub payload_lengths: Vec<u64>,
    pub source_size: u64,
    pub caution: String,
}

fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{b:02X}")).collect()
}

fn context_hash(context: &DecodeContext) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(CONTEXT_DOMAIN);
    h.update(context.canonical_bytes());
    h.finalize().into()
}

fn framed_payload_hash<R: Read + Seek>(
    reader: &mut R,
    ranges: &[(u64, u64)],
) -> Result<([u8; 32], u64)> {
    let mut h = Sha256::new();
    h.update(PAYLOAD_DOMAIN);
    h.update((ranges.len() as u64).to_le_bytes());
    let mut scratch = vec![0_u8; 1024 * 1024];
    let mut total = 0_u64;
    for (offset, count) in ranges {
        reader.seek(SeekFrom::Start(*offset))?;
        h.update(count.to_le_bytes());
        let mut remaining = *count;
        while remaining > 0 {
            let n = remaining.min(scratch.len() as u64) as usize;
            reader
                .read_exact(&mut scratch[..n])
                .context("raw payload truncated during proof read")?;
            h.update(&scratch[..n]);
            remaining -= n as u64;
        }
        total = total.checked_add(*count).context("payload size overflow")?;
    }
    Ok((h.finalize().into(), total))
}

pub fn input_proof(
    path: &Path,
    linearization_count: Option<usize>,
    raw_ifd_offset: u32,
) -> Result<InputProof> {
    // Identical prepare() to this isolated copy's full pixel-digest path.
    let Prepared {
        mut reader,
        metadata,
        ifd,
        width,
        height,
        linearization_count,
    } = prepare(path, linearization_count, raw_ifd_offset)?;
    let result = proof_from_prepared(
        &ifd,
        width,
        height,
        linearization_count,
        metadata.len(),
        &mut reader,
    )?;
    ensure!(
        same_file_state(&metadata, &reader.get_ref().metadata()?),
        "source changed during proof read"
    );
    Ok(result)
}

pub(super) fn proof_from_prepared<R: Read + Seek>(
    ifd: &IFD,
    width: u32,
    height: u32,
    linearization_count: Option<usize>,
    source_size: u64,
    reader: &mut R,
) -> Result<InputProof> {
    let tiled = ifd.entries.contains_key(&324);
    let offset_tag = if tiled { 324 } else { 273 };
    let count_tag = if tiled { 325 } else { 279 };
    // Rawler's strip_data/tile_data require LONG arrays even though our general
    // bounds helper accepts SHORT. Never establish a proof for a decode failure.
    for tag in [offset_tag, count_tag] {
        ensure!(
            matches!(
                ifd.entries.get(&tag).map(|e| &e.value),
                Some(Value::Long(_))
            ),
            "Rawler requires LONG payload arrays"
        );
    }
    let offsets = array(&ifd, offset_tag)?;
    let counts = array(&ifd, count_tag)?;
    let contiguous = !tiled
        && offsets
            .windows(2)
            .enumerate()
            .all(|(i, pair)| pair[0].checked_add(counts[i]) == Some(pair[1]));
    let context = DecodeContext {
        width,
        height,
        little_endian: matches!(ifd.endian, Endian::Little),
        photometric: scalar(&ifd, 262, None)?,
        samples_per_pixel: scalar(&ifd, 277, Some(1))?,
        planar_configuration: scalar(&ifd, 284, Some(1))?,
        sample_format: scalar(&ifd, 339, Some(1))?,
        compression: scalar(&ifd, 259, Some(1))?,
        bits_per_sample: scalar(&ifd, 258, None)?,
        row_interleave: scalar(&ifd, 50975, Some(1))?,
        column_interleave: scalar(&ifd, 52547, Some(1))?,
        linearization_count,
        adobe_digest_uses_byte_samples: linearization_count.is_some_and(|n| n <= 256),
        tiled,
        rows_per_strip: if tiled {
            0
        } else {
            scalar(&ifd, 278, Some(height))?
        },
        tile_width: if tiled { scalar(&ifd, 322, None)? } else { 0 },
        tile_height: if tiled { scalar(&ifd, 323, None)? } else { 0 },
        strip_payloads_physically_contiguous: contiguous,
        payload_count: offsets.len(),
    };
    let context_sha = context_hash(&context);
    let ranges: Vec<_> = offsets.into_iter().zip(counts.iter().copied()).collect();
    let (payload_sha, payload_bytes) = framed_payload_hash(reader, &ranges)?;
    let mut h = Sha256::new();
    h.update(PROOF_DOMAIN);
    h.update(context_sha);
    h.update(payload_sha);
    let combined: [u8; 32] = h.finalize().into();
    Ok(InputProof {
        schema: "raw-decode-equivalence-proof-v1-experimental".into(),
        context,
        context_sha256: hex(&context_sha),
        framed_payload_sha256: hex(&payload_sha),
        combined_sha256: hex(&combined),
        payload_bytes,
        payload_lengths: counts,
        source_size,
        caution: "Requires an unchanged remote source with this exact proof AND a successful full v5 decode; this proof alone does not decode/validate JPEG entropy and is never a standalone Lightroom fingerprint".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn context() -> DecodeContext {
        DecodeContext {
            width: 4,
            height: 4,
            little_endian: true,
            photometric: 32803,
            samples_per_pixel: 1,
            planar_configuration: 1,
            sample_format: 1,
            compression: 1,
            bits_per_sample: 16,
            row_interleave: 1,
            column_interleave: 1,
            linearization_count: None,
            adobe_digest_uses_byte_samples: false,
            tiled: false,
            rows_per_strip: 2,
            tile_width: 0,
            tile_height: 0,
            strip_payloads_physically_contiguous: true,
            payload_count: 2,
        }
    }

    #[test]
    fn every_context_word_is_bound() {
        let c = context();
        let bytes = c.canonical_bytes();
        let baseline = context_hash(&c);
        for index in 0..20 {
            let mut mutated = bytes.clone();
            mutated[index * 8] ^= 1;
            let mut h = Sha256::new();
            h.update(CONTEXT_DOMAIN);
            h.update(mutated);
            let changed: [u8; 32] = h.finalize().into();
            assert_ne!(baseline, changed, "word {index}");
        }
        let mut changed = c.clone();
        changed.strip_payloads_physically_contiguous = false;
        assert_ne!(context_hash(&c), context_hash(&changed));
        changed = c.clone();
        changed.linearization_count = Some(256);
        changed.adobe_digest_uses_byte_samples = true;
        assert_ne!(context_hash(&c), context_hash(&changed));
    }

    #[test]
    fn offset_relocation_and_unrelated_metadata_do_not_change_payload() {
        let a = framed_payload_hash(&mut Cursor::new(b"metaABCDEF"), &[(4, 3), (7, 3)])
            .unwrap()
            .0;
        let b = framed_payload_hash(
            &mut Cursor::new(b"rewritten-XMP-ABCDEFextra"),
            &[(14, 3), (17, 3)],
        )
        .unwrap()
        .0;
        assert_eq!(a, b);
    }

    #[test]
    fn payload_byte_order_and_segment_boundaries_are_bound() {
        let bytes = b"ABCDEF";
        let a = framed_payload_hash(&mut Cursor::new(bytes), &[(0, 3), (3, 3)])
            .unwrap()
            .0;
        for ranges in [&[(3, 3), (0, 3)][..], &[(0, 2), (2, 4)][..], &[(0, 6)][..]] {
            assert_ne!(
                a,
                framed_payload_hash(&mut Cursor::new(bytes), ranges)
                    .unwrap()
                    .0
            );
        }
        assert_ne!(
            a,
            framed_payload_hash(&mut Cursor::new(b"ABCDEX"), &[(0, 3), (3, 3)])
                .unwrap()
                .0
        );
    }

    #[test]
    fn truncated_payload_fails() {
        assert!(framed_payload_hash(&mut Cursor::new(b"ABC"), &[(0, 4)]).is_err());
    }

    fn fixture(
        ifd_offset: u32,
        payload_offset: u32,
        xmp: [u8; 4],
        changed_tag: Option<(u16, u32)>,
    ) -> std::path::PathBuf {
        use std::io::Write;
        let mut bytes = vec![0_u8; payload_offset as usize + 32];
        bytes[..8].copy_from_slice(&[b'I', b'I', 42, 0, 0, 0, 0, 0]);
        bytes[4..8].copy_from_slice(&ifd_offset.to_le_bytes());
        let fields = [
            (256, 4),
            (257, 4),
            (258, 16),
            (259, 1),
            (262, 32803),
            (273, payload_offset),
            (277, 1),
            (278, 4),
            (279, 32),
            (284, 1),
            (339, 1),
            (700, u32::from_le_bytes(xmp)),
        ];
        let start = ifd_offset as usize;
        bytes[start..start + 2].copy_from_slice(&(fields.len() as u16).to_le_bytes());
        for (i, (tag, value)) in fields.into_iter().enumerate() {
            let value = changed_tag
                .filter(|(t, _)| *t == tag)
                .map_or(value, |(_, v)| v);
            let at = start + 2 + i * 12;
            bytes[at..at + 2].copy_from_slice(&tag.to_le_bytes());
            bytes[at + 2..at + 4].copy_from_slice(&4_u16.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&1_u32.to_le_bytes());
            bytes[at + 8..at + 12].copy_from_slice(&value.to_le_bytes());
        }
        for i in 0..16 {
            bytes[payload_offset as usize + i * 2..payload_offset as usize + i * 2 + 2]
                .copy_from_slice(&(i as u16).to_le_bytes());
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/retained-test-fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}-{stamp}.tiff", std::process::id()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        path
    }

    #[test]
    fn actual_ifd_and_payload_relocation_matches_full_decode() {
        let a = fixture(8, 256, *b"old!", None);
        let b = fixture(32, 512, *b"new!", None);
        let pa = input_proof(&a, None, 8).unwrap();
        let pb = input_proof(&b, None, 32).unwrap();
        assert_eq!(pa.combined_sha256, pb.combined_sha256);
        assert_eq!(
            compute(&a, None, 8).unwrap().digest,
            compute(&b, None, 32).unwrap().digest
        );
    }

    #[test]
    fn meaningful_ifd_changes_reject_or_change_proof() {
        let a = fixture(8, 256, *b"same", None);
        let baseline = input_proof(&a, None, 8).unwrap();
        for field in [
            (256, 2),
            (257, 2),
            (258, 8),
            (259, 7),
            (262, 34892),
            (277, 2),
            (278, 2),
            (279, 30),
            (284, 2),
            (339, 2),
        ] {
            let b = fixture(8, 256, *b"same", Some(field));
            if let Ok(p) = input_proof(&b, None, 8) {
                assert_ne!(
                    baseline.combined_sha256, p.combined_sha256,
                    "tag {}",
                    field.0
                );
            }
        }
    }
}
