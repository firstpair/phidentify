//! Bounded metadata reader for ordinary single-plane CFA or monochrome DNG.
//! Does not decode pixels, compute the raw digest, or modify any file.

use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_IFDS: usize = 64;
const MAX_ENTRIES: usize = 8192;
const MAX_SELECTED_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_METADATA: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DngMetadata {
    pub model: String,
    pub crop_size: [(u32, u32); 2],
    pub crop_origin: [(u32, u32); 2],
    pub user_crop: Option<[(u32, u32); 4]>,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub raw_ifd_offset: u32,
    pub linearization_count: Option<usize>,
    /// Typed little-endian list1/list2/list2 bytes from the original raw IFD.
    pub opcode_fingerprint_suffix: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn u16(self, b: &[u8]) -> u16 {
        let v = [b[0], b[1]];
        match self {
            Self::Little => u16::from_le_bytes(v),
            Self::Big => u16::from_be_bytes(v),
        }
    }
    fn u32(self, b: &[u8]) -> u32 {
        let v = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => u32::from_le_bytes(v),
            Self::Big => u32::from_be_bytes(v),
        }
    }
}

struct Field {
    kind: u16,
    count: u32,
    bytes: Vec<u8>,
}

impl Field {
    fn integers(&self, endian: Endian) -> Result<Vec<u32>> {
        match self.kind {
            3 => Ok(self
                .bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u32::from(endian.u16(b)))
                .collect()),
            4 | 13 => Ok(self
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| endian.u32(b))
                .collect()),
            _ => bail!(
                "expected unsigned SHORT/LONG/IFD metadata, got TIFF type {}",
                self.kind
            ),
        }
    }
    fn scalar(&self, endian: Endian) -> Result<u32> {
        ensure!(self.count == 1, "expected scalar metadata");
        Ok(self.integers(endian)?[0])
    }
    fn rationals<const N: usize>(&self, endian: Endian) -> Result<[(u32, u32); N]> {
        ensure!(
            usize::try_from(self.count)? == N,
            "expected {N} rational values"
        );
        let values = match self.kind {
            3 | 4 => self
                .integers(endian)?
                .into_iter()
                .map(|n| (n, 1))
                .collect::<Vec<_>>(),
            5 => self
                .bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| (endian.u32(b), endian.u32(&b[4..])))
                .collect(),
            _ => bail!(
                "unsupported crop metadata type {}; exact unsigned rationals required",
                self.kind
            ),
        };
        ensure!(values.iter().all(|(_, d)| *d != 0), "zero crop denominator");
        values
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid rational count"))
    }
}

struct Ifd {
    offset: u64,
    fields: BTreeMap<u16, Field>,
}

fn width(kind: u16) -> Result<u64> {
    match kind {
        1 | 2 | 6 | 7 => Ok(1),
        3 | 8 => Ok(2),
        4 | 9 | 11 | 13 => Ok(4),
        5 | 10 | 12 => Ok(8),
        _ => bail!("unsupported TIFF metadata type {kind}"),
    }
}

fn range(start: u64, len: u64, file_len: u64) -> Result<()> {
    ensure!(
        start.checked_add(len).is_some_and(|end| end <= file_len),
        "TIFF metadata range exceeds file bounds"
    );
    Ok(())
}

fn read_at<R: Read + Seek>(reader: &mut R, position: u64, bytes: &mut [u8]) -> Result<()> {
    reader.seek(SeekFrom::Start(position))?;
    reader.read_exact(bytes).context("truncated TIFF metadata")
}

fn selected(tag: u16) -> bool {
    matches!(
        tag,
        254 | 256
            | 257
            | 258
            | 259
            | 262
            | 277
            | 330
            | 339
            | 0xc612
            | 0xc614
            | 0xc618
            | 0xc61f
            | 0xc620
            | 0xc68d
            | 0xc740
            | 0xc741
            | 0xc74e
            | 0xc7b5
            | 0xc7ee
    )
}

fn required(ifd: &Ifd, tag: u16) -> Result<&Field> {
    ifd.fields
        .get(&tag)
        .with_context(|| format!("required DNG tag 0x{tag:04x} absent in IFD {}", ifd.offset))
}

fn validate_ignored_opcode3(field: &Field) -> Result<()> {
    ensure!(
        matches!(field.kind, 1 | 7) && field.bytes.len() >= 4,
        "malformed OpcodeList3"
    );
    let bytes = &field.bytes;
    let count = u32::from_be_bytes(bytes[..4].try_into()?);
    ensure!(count <= 4096, "too many OpcodeList3 entries");
    let mut position = 4usize;
    for _ in 0..count {
        ensure!(
            position
                .checked_add(16)
                .is_some_and(|end| end <= bytes.len()),
            "truncated OpcodeList3 header"
        );
        let id = u32::from_be_bytes(bytes[position..position + 4].try_into()?);
        let version = &bytes[position + 4..position + 8];
        let flags = u32::from_be_bytes(bytes[position + 8..position + 12].try_into()?);
        let length = u32::from_be_bytes(bytes[position + 12..position + 16].try_into()?);
        ensure!((1..=14).contains(&id), "unverified OpcodeList3 opcode ID");
        ensure!(
            version[0] == 1 && (3..=7).contains(&version[1]),
            "unverified OpcodeList3 minimum version"
        );
        ensure!(flags & !3 == 0, "unverified OpcodeList3 flags");
        position = position
            .checked_add(16)
            .and_then(|p| p.checked_add(length as usize))
            .context("OpcodeList3 size overflow")?;
        ensure!(
            position <= bytes.len(),
            "OpcodeList3 data exceeds tag bounds"
        );
    }
    ensure!(
        position == bytes.len(),
        "unexpected trailing OpcodeList3 bytes"
    );
    Ok(())
}

fn user_crop(field: &Field, endian: Endian) -> Result<Option<[(u32, u32); 4]>> {
    let v = field.rationals::<4>(endian)?;
    ensure!(
        v.iter().all(|(n, d)| n <= d),
        "DefaultUserCrop coordinate outside 0..1"
    );
    ensure!(
        u64::from(v[0].0) * u64::from(v[2].1) < u64::from(v[2].0) * u64::from(v[0].1)
            && u64::from(v[1].0) * u64::from(v[3].1) < u64::from(v[3].0) * u64::from(v[1].1),
        "empty or inverted DefaultUserCrop"
    );
    if v[0].0 == 0 && v[1].0 == 0 && v[2].0 == v[2].1 && v[3].0 == v[3].1 {
        Ok(None)
    } else {
        Ok(Some(v))
    }
}

fn parse<R: Read + Seek>(reader: &mut R, file_len: u64) -> Result<DngMetadata> {
    range(0, 8, file_len)?;
    let mut header = [0u8; 8];
    read_at(reader, 0, &mut header)?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => bail!("not a TIFF byte-order header"),
    };
    ensure!(
        endian.u16(&header[2..4]) == 42,
        "only classic TIFF DNG is supported (not RW2 or BigTIFF)"
    );
    let first = u64::from(endian.u32(&header[4..8]));
    ensure!(first >= 8, "missing or invalid IFD0");
    let mut queue = VecDeque::from([first]);
    let mut seen = HashSet::new();
    let mut ifds = Vec::new();
    let mut total = 0usize;
    while let Some(offset) = queue.pop_front() {
        ensure!(
            offset >= 8 && seen.insert(offset),
            "cyclic, repeated or invalid IFD reference"
        );
        ensure!(seen.len() <= MAX_IFDS, "too many image IFDs");
        range(offset, 2, file_len)?;
        let mut n = [0u8; 2];
        read_at(reader, offset, &mut n)?;
        let count = usize::from(endian.u16(&n));
        ensure!(count <= MAX_ENTRIES, "too many IFD entries");
        let length = 2 + 12 * count + 4;
        range(offset, length as u64, file_len)?;
        let mut entries = vec![0u8; length];
        read_at(reader, offset, &mut entries)?;
        let mut fields = BTreeMap::new();
        for entry in entries[2..2 + 12 * count].as_chunks::<12>().0 {
            let tag = endian.u16(entry);
            let kind = endian.u16(&entry[2..]);
            let count = endian.u32(&entry[4..]);
            let size = u64::from(count)
                .checked_mul(width(kind)?)
                .context("field size overflow")?;
            let external = (size > 4).then(|| u64::from(endian.u32(&entry[8..])));
            if let Some(location) = external {
                ensure!(location >= 8, "field points into header");
                range(location, size, file_len)?;
            }
            if !selected(tag) {
                continue;
            }
            ensure!(
                !fields.contains_key(&tag),
                "duplicate relevant tag 0x{tag:04x}"
            );
            ensure!(
                size <= MAX_SELECTED_BYTES,
                "selected metadata field exceeds 1 MiB"
            );
            total = total
                .checked_add(usize::try_from(size)?)
                .context("metadata size overflow")?;
            ensure!(
                total <= MAX_TOTAL_METADATA,
                "selected metadata exceeds 8 MiB"
            );
            let mut bytes = vec![0u8; usize::try_from(size)?];
            if let Some(location) = external {
                read_at(reader, location, &mut bytes)?;
            } else {
                bytes.copy_from_slice(&entry[8..8 + size as usize]);
            }
            fields.insert(tag, Field { kind, count, bytes });
        }
        if let Some(children) = fields.get(&330) {
            for child in children.integers(endian)? {
                ensure!(child >= 8, "invalid SubIFD");
                queue.push_back(u64::from(child));
            }
        }
        let next = endian.u32(&entries[2 + 12 * count..]);
        if next != 0 {
            queue.push_back(u64::from(next));
        }
        ifds.push(Ifd { offset, fields });
    }
    let ifd0 = ifds.first().context("missing IFD0")?;
    let version = required(ifd0, 0xc612)?;
    ensure!(
        version.kind == 1 && version.count == 4,
        "invalid DNGVersion"
    );
    ensure!(
        version.bytes[0] == 1 && version.bytes[1] <= 7,
        "unverified DNG version"
    );
    let model = required(ifd0, 0xc614)?;
    ensure!(
        model.kind == 2 && model.bytes.len() > 1 && model.bytes.last() == Some(&0),
        "UniqueCameraModel must be a nonempty NUL-terminated string"
    );
    let model = &model.bytes[..model.bytes.len() - 1];
    ensure!(!model.contains(&0), "interior NUL in UniqueCameraModel");
    let model = std::str::from_utf8(model)
        .context("UniqueCameraModel is not UTF-8")?
        .to_owned();
    let mut raw = None;
    for ifd in &ifds {
        if let Some(enhancement) = ifd.fields.get(&0xc7ee) {
            ensure!(
                enhancement.kind == 2 && enhancement.bytes.iter().all(|byte| *byte == 0),
                "nonempty or malformed EnhanceParams is unsupported"
            );
        }
        let subtype = required(ifd, 254)?.scalar(endian)?;
        let photometric = required(ifd, 262)?.scalar(endian)?;
        if subtype == 0 && matches!(photometric, 32803 | 34892) {
            ensure!(
                raw.is_none(),
                "multiple full-resolution raw image IFDs are unsupported"
            );
            raw = Some(ifd);
        } else {
            ensure!(
                subtype == 1 && matches!(photometric, 2 | 6 | 34892),
                "enhanced, additional-plane or nonordinary image IFD is unsupported"
            );
            // Reduced LinearRaw images are previews, not additional original
            // planes. The inspected Adobe previews have three color samples.
            if photometric == 34892 {
                ensure!(
                    required(ifd, 277)?.scalar(endian)? == 3,
                    "unverified reduced LinearRaw preview sample layout"
                );
            }
        }
    }
    let raw = raw.context("no full-resolution CFA or LinearRaw image IFD")?;
    // Adobe dng_negative::ReadStage1Image takes opcode lists exclusively from
    // info.fIFD[info.fMainIndex]. Reduced previews can carry independent
    // rendering opcodes; they do not enter the original-image fingerprint.
    let opcode_bytes = |tag| -> Result<Option<&[u8]>> {
        raw.fields
            .get(&tag)
            .map(|opcode| {
                ensure!(matches!(opcode.kind, 1 | 7), "invalid raw opcode list type");
                Ok(opcode.bytes.as_slice())
            })
            .transpose()
    };
    // The typed serializer rejects every unverified opcode family. Tag byte
    // order is big-endian independently of this TIFF's byte order.
    let opcode_fingerprint_suffix =
        crate::lightroom::opcodes::installed_15_6_suffix(opcode_bytes(0xc740)?, opcode_bytes(0xc741)?)?;
    // Installed Lightroom 15.6 hashes lists 1,2,2, omitting list 3.
    // Two native controls independently confirmed this. Validate its
    // container, without applying opcodes or serializing it into the hash.
    if let Some(opcode) = raw.fields.get(&0xc74e) {
        validate_ignored_opcode3(opcode)?;
    }
    let sensor_width = required(raw, 256)?.scalar(endian)?;
    let sensor_height = required(raw, 257)?.scalar(endian)?;
    ensure!(sensor_width > 0 && sensor_height > 0, "zero raw dimensions");
    ensure!(
        matches!(required(raw, 259)?.scalar(endian)?, 1 | 7),
        "unverified raw compression (only uncompressed/lossless JPEG supported)"
    );
    ensure!(
        required(raw, 277)?.scalar(endian)? == 1,
        "multiple raw samples/planes are unsupported"
    );
    ensure!(
        (1..=16).contains(&required(raw, 258)?.scalar(endian)?),
        "raw BitsPerSample must be an integer in 1..=16"
    );
    if let Some(format) = raw.fields.get(&339) {
        ensure!(
            format.scalar(endian)? == 1,
            "signed or floating-point samples are unsupported"
        );
    }
    // Adobe DNG SDK dng_ifd::PostParse: zero/absent ActiveArea becomes the
    // full image; absent crop size uses ActiveArea dimensions. The constructor
    // initializes DefaultCropOrigin to (0/1,0/1), relative to ActiveArea.
    let mut active_area = if let Some(area) = raw.fields.get(&0xc68d) {
        ensure!(
            area.count == 4 && matches!(area.kind, 3 | 4),
            "invalid ActiveArea"
        );
        let v = area.integers(endian)?;
        [v[0], v[1], v[2], v[3]]
    } else {
        [0, 0, sensor_height, sensor_width]
    };
    if active_area == [0; 4] {
        active_area = [0, 0, sensor_height, sensor_width];
    }
    ensure!(
        active_area[0] < active_area[2]
            && active_area[1] < active_area[3]
            && active_area[2] <= sensor_height
            && active_area[3] <= sensor_width,
        "ActiveArea outside raw dimensions"
    );
    let crop_size = raw
        .fields
        .get(&0xc620)
        .map(|f| f.rationals::<2>(endian))
        .transpose()?
        .unwrap_or([
            (active_area[3] - active_area[1], 1),
            (active_area[2] - active_area[0], 1),
        ]);
    let crop_origin = raw
        .fields
        .get(&0xc61f)
        .map(|f| f.rationals::<2>(endian))
        .transpose()?
        .unwrap_or([(0, 1); 2]);
    let user_crop = raw
        .fields
        .get(&0xc7b5)
        .map(|f| user_crop(f, endian))
        .transpose()?
        .flatten();
    for ifd in &ifds {
        if ifd.offset != raw.offset
            && let Some(crop) = ifd.fields.get(&0xc7b5)
        {
            ensure!(
                self::user_crop(crop, endian)?.is_none(),
                "nondefault user crop outside raw IFD is unverified"
            );
        }
    }
    ensure!(
        crop_size.iter().all(|(n, _)| *n > 0),
        "zero default crop size"
    );
    let linearization_count = raw
        .fields
        .get(&0xc618)
        .map(|field| {
            ensure!(
                field.kind == 3 && (1..=65536).contains(&field.count),
                "invalid LinearizationTable"
            );
            Ok(usize::try_from(field.count)?)
        })
        .transpose()?;
    Ok(DngMetadata {
        model,
        crop_size,
        crop_origin,
        user_crop,
        sensor_width,
        sensor_height,
        raw_ifd_offset: u32::try_from(raw.offset)?,
        linearization_count,
        opcode_fingerprint_suffix,
    })
}

/// Read only the selected metadata from an ordinary single-plane raw DNG.
pub fn read(path: &Path) -> Result<DngMetadata> {
    let file = File::open(path).with_context(|| format!("open DNG metadata {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "DNG input must be a regular file");
    parse(&mut BufReader::new(file), metadata.len())
        .with_context(|| format!("read DNG metadata {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(big: bool, mutate: impl FnOnce(&mut BTreeMap<u16, (u16, u32, Vec<u8>)>)) -> Vec<u8> {
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes().to_vec()
            } else {
                v.to_le_bytes().to_vec()
            }
        };
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes().to_vec()
            } else {
                v.to_le_bytes().to_vec()
            }
        };
        let rationals = |values: &[(u32, u32)]| {
            values
                .iter()
                .flat_map(|(n, d)| [u32b(*n), u32b(*d)].concat())
                .collect()
        };
        let mut fields = BTreeMap::from([
            (254, (4, 1, u32b(0))),
            (256, (4, 1, u32b(64))),
            (257, (4, 1, u32b(48))),
            (258, (3, 1, u16b(14))),
            (259, (3, 1, u16b(7))),
            (262, (3, 1, u16b(32803))),
            (277, (3, 1, u16b(1))),
            (0xc612, (1, 4, vec![1, 4, 0, 0])),
            (0xc614, (2, 8, b"Fixture\0".to_vec())),
            (0xc61f, (5, 2, rationals(&[(0, 3), (4, 2)]))),
            (0xc620, (5, 2, rationals(&[(120, 2), (40, 1)]))),
        ]);
        mutate(&mut fields);
        let mut data = if big { b"MM".to_vec() } else { b"II".to_vec() };
        data.extend(u16b(42));
        data.extend(u32b(8));
        data.extend(u16b(fields.len() as u16));
        let mut next = 8 + 2 + 12 * fields.len() + 4;
        let mut external = Vec::new();
        for (tag, (kind, count, bytes)) in fields {
            data.extend(u16b(tag));
            data.extend(u16b(kind));
            data.extend(u32b(count));
            if bytes.len() > 4 {
                data.extend(u32b(next as u32));
                next += bytes.len();
                external.extend(bytes);
            } else {
                let mut inline = [0u8; 4];
                inline[..bytes.len()].copy_from_slice(&bytes);
                data.extend(inline);
            }
        }
        data.extend(u32b(0));
        data.extend(external);
        data
    }

    fn run(bytes: Vec<u8>) -> Result<DngMetadata> {
        let len = bytes.len() as u64;
        parse(&mut Cursor::new(bytes), len)
    }

    fn append_image_ifd(
        mut bytes: Vec<u8>,
        subtype: u32,
        photometric: u16,
        samples: u16,
        opcode_tag: u16,
    ) -> Vec<u8> {
        // Little-endian fixture with a separate, bounded active MapPolynomial
        // list. The payload belongs to this image IFD alone.
        let main_count = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
        let next_pointer = 10 + main_count * 12;
        let offset = bytes.len() as u32;
        bytes[next_pointer..next_pointer + 4].copy_from_slice(&offset.to_le_bytes());
        let opcode = [1u32, 8, 0x01030000, 0, 68]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .chain([0u8; 68])
            .collect::<Vec<_>>();
        let fields = [
            (254u16, 4u16, 1u32, subtype),
            (256, 4, 1, 16),
            (257, 4, 1, 12),
            (262, 3, 1, u32::from(photometric)),
            (277, 3, 1, u32::from(samples)),
            (opcode_tag, 7, opcode.len() as u32, offset + 2 + 6 * 12 + 4),
        ];
        bytes.extend((fields.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in fields {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(opcode);
        bytes
    }

    #[test]
    fn reduced_preview_opcodes_do_not_change_main_image_metadata() {
        let baseline = run(fixture(false, |_| {})).unwrap();
        for photometric in [2, 6, 34892] {
            for tag in [0xc740, 0xc741, 0xc74e] {
                let bytes = append_image_ifd(fixture(false, |_| {}), 1, photometric, 3, tag);
                assert_eq!(run(bytes).unwrap(), baseline);
            }
        }
        // A malformed list on the main image must still fail closed even when
        // another IFD has a valid preview classification.
        for tag in [0xc740, 0xc741] {
            let main = fixture(false, |f| {
                f.insert(tag, (7, 4, 1u32.to_be_bytes().to_vec()));
            });
            assert!(run(append_image_ifd(main, 1, 34892, 3, 0xc741)).is_err());
        }
    }

    #[test]
    fn preview_scope_keeps_extra_main_mask_depth_and_enhanced_gates() {
        for (subtype, photometric, samples) in [
            (0, 32803, 1),  // another full raw image
            (0, 34892, 3),  // another full linear image
            (4, 4, 1),      // transparency mask
            (8, 51177, 1),  // depth image
            (16, 34892, 3), // enhanced image
            (1, 34892, 1),  // unverified reduced linear sample layout
        ] {
            assert!(
                run(append_image_ifd(
                    fixture(false, |_| {}),
                    subtype,
                    photometric,
                    samples,
                    0xc741,
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn preserves_endian_and_unreduced_rationals() {
        for big in [false, true] {
            let m = run(fixture(big, |_| {})).unwrap();
            assert_eq!(m.model, "Fixture");
            assert_eq!(m.sensor_width, 64);
            assert_eq!(m.sensor_height, 48);
            assert_eq!(m.crop_size, [(120, 2), (40, 1)]);
            assert_eq!(m.crop_origin, [(0, 3), (4, 2)]);
            assert_eq!(m.linearization_count, None);
        }
    }

    #[test]
    fn typed_raw_opcodes_are_scoped_to_main_ifd_and_independent_of_tiff_endian() {
        let one: Vec<u8> = [1u32, 4, 0x01030000, 0, 8, 0, 2]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let mut two: Vec<u8> = [1u32, 3, 0x01030000, 1, 56]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        for value in [0.0f64, 0., 0., 0., 0., 0.5, 0.5] {
            two.extend(value.to_bits().to_be_bytes());
        }
        let expected = crate::lightroom::opcodes::installed_15_6_suffix(Some(&one), Some(&two)).unwrap();
        for big in [false, true] {
            let m = run(fixture(big, |f| {
                f.insert(0xc740, (7, one.len() as u32, one.clone()));
                f.insert(0xc741, (7, two.len() as u32, two.clone()));
            }))
            .unwrap();
            assert_eq!(m.opcode_fingerprint_suffix, expected);
        }
        let main = fixture(false, |f| {
            f.insert(0xc740, (7, one.len() as u32, one.clone()));
            f.insert(0xc741, (7, two.len() as u32, two.clone()));
        });
        assert_eq!(
            run(append_image_ifd(main, 1, 34892, 3, 0xc741))
                .unwrap()
                .opcode_fingerprint_suffix,
            expected
        );
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc740, (2, one.len() as u32, one));
            }))
            .is_err()
        );
        let unknown: Vec<u8> = [1u32, 8, 0x01030000, 0, 68]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .chain([0u8; 68])
            .collect();
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc741, (7, unknown.len() as u32, unknown));
            }))
            .is_err()
        );
    }
    #[test]
    fn promotes_short_and_long_crop_without_guessing_defaults() {
        let m = run(fixture(false, |f| {
            f.insert(
                0xc620,
                (3, 2, [60u16.to_le_bytes(), 40u16.to_le_bytes()].concat()),
            );
            f.insert(
                0xc61f,
                (4, 2, [0u32.to_le_bytes(), 2u32.to_le_bytes()].concat()),
            );
        }))
        .unwrap();
        assert_eq!(m.crop_size, [(60, 1), (40, 1)]);
        assert_eq!(m.crop_origin, [(0, 1), (2, 1)]);
        assert_eq!(
            run(fixture(false, |f| {
                f.remove(&0xc61f);
            }))
            .unwrap()
            .crop_origin,
            [(0, 1); 2]
        );
        assert!(
            run(fixture(false, |f| {
                f.remove(&0xc614);
            }))
            .is_err()
        );
    }
    #[test]
    fn rejects_unverified_sample_types_compression_and_image_planes() {
        for (tag, value) in [(259, 52546u16), (258, 32), (277, 3), (339, 2), (339, 3)] {
            assert!(
                run(fixture(false, |f| {
                    f.insert(tag, (3, 1, value.to_le_bytes().to_vec()));
                }))
                .is_err()
            );
        }
        assert!(
            run(fixture(false, |f| {
                f.insert(254, (4, 1, 16u32.to_le_bytes().to_vec()));
            }))
            .is_err()
        );
    }

    #[test]
    fn accepts_single_plane_monochrome_but_rejects_three_plane_linear_raw() {
        let monochrome = fixture(false, |f| {
            f.insert(262, (3, 1, 34892u16.to_le_bytes().to_vec()));
        });
        assert!(run(monochrome).is_ok());
        let color_linear = fixture(false, |f| {
            f.insert(262, (3, 1, 34892u16.to_le_bytes().to_vec()));
            f.insert(277, (3, 1, 3u16.to_le_bytes().to_vec()));
        });
        assert!(run(color_linear).is_err());
    }
    #[test]
    fn rejects_enhanced_malformed_opcodes_and_zero_denominators_but_preserves_user_crop() {
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc7ee, (2, 8, b"denoise\0".to_vec()));
            }))
            .is_err()
        );
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc740, (7, 4, 1u32.to_be_bytes().to_vec()));
            }))
            .is_err()
        );
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc740, (7, 4, 0u32.to_be_bytes().to_vec()));
            }))
            .is_ok()
        );
        let values = [0u32, 1, 0, 1, 1, 2, 1, 1];
        let cropped = run(fixture(false, |f| {
            f.insert(
                0xc7b5,
                (
                    5,
                    4,
                    values.into_iter().flat_map(u32::to_le_bytes).collect(),
                ),
            );
        }))
        .unwrap();
        assert_eq!(cropped.user_crop, Some([(0, 1), (0, 1), (1, 2), (1, 1)]));
        assert!(
            run(fixture(false, |f| {
                f.get_mut(&0xc620).unwrap().2[4..8].fill(0);
            }))
            .is_err()
        );
    }
    #[test]
    fn bounds_metadata_and_detects_ifd_cycles() {
        let mut truncated = fixture(false, |_| {});
        truncated.truncate(20);
        assert!(run(truncated).is_err());
        assert!(
            run(fixture(false, |f| {
                f.insert(330, (4, 1, 8u32.to_le_bytes().to_vec()));
            }))
            .is_err()
        );
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc618, (3, 65537, vec![0; 131074]));
            }))
            .is_err()
        );
    }

    #[test]
    fn adobe_crop_defaults_use_active_area_and_zero_origin() {
        let full = run(fixture(false, |f| {
            f.remove(&0xc620);
            f.remove(&0xc61f);
        }))
        .unwrap();
        assert_eq!(full.crop_size, [(64, 1), (48, 1)]);
        assert_eq!(full.crop_origin, [(0, 1); 2]);
        let active = run(fixture(false, |f| {
            f.remove(&0xc620);
            f.remove(&0xc61f);
            f.insert(
                0xc68d,
                (
                    4,
                    4,
                    [2u32, 4, 46, 62]
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect(),
                ),
            );
        }))
        .unwrap();
        assert_eq!(active.crop_size, [(58, 1), (44, 1)]);
        assert_eq!(active.crop_origin, [(0, 1); 2]);
        let zero = run(fixture(false, |f| {
            f.remove(&0xc620);
            f.insert(0xc68d, (4, 4, vec![0; 16]));
        }))
        .unwrap();
        assert_eq!(zero.crop_size, full.crop_size);
        assert!(
            run(fixture(false, |f| {
                f.insert(
                    0xc68d,
                    (
                        4,
                        4,
                        [0u32, 0, 49, 64]
                            .into_iter()
                            .flat_map(u32::to_le_bytes)
                            .collect(),
                    ),
                );
            }))
            .is_err()
        );
    }

    #[test]
    fn user_crop_preserves_exact_nondefault_ratios() {
        let ratios = [0u32, 4000, 336, 6000, 4000, 4000, 5664, 6000];
        let metadata = run(fixture(false, |f| {
            f.insert(
                0xc7b5,
                (
                    5,
                    4,
                    ratios.into_iter().flat_map(u32::to_le_bytes).collect(),
                ),
            );
        }))
        .unwrap();
        assert_eq!(
            metadata.user_crop,
            Some([(0, 4000), (336, 6000), (4000, 4000), (5664, 6000)])
        );
        let default = [0u32, 4000, 0, 6000, 4000, 4000, 6000, 6000];
        let metadata = run(fixture(false, |f| {
            f.insert(
                0xc7b5,
                (
                    5,
                    4,
                    default.into_iter().flat_map(u32::to_le_bytes).collect(),
                ),
            );
        }))
        .unwrap();
        assert_eq!(metadata.user_crop, None);
    }

    #[test]
    fn ignored_opcode3_still_requires_complete_bounded_container() {
        let make = |id: u32, version: u32, flags: u32, length: u32| {
            [1u32, id, version, flags, length, 0x01020304]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect::<Vec<_>>()
        };
        let valid = make(1, 0x01030000, 0, 4);
        assert!(
            run(fixture(false, |f| {
                f.insert(0xc74e, (7, valid.len() as u32, valid.clone()));
            }))
            .is_ok()
        );
        assert!(
            run(fixture(true, |f| {
                f.insert(0xc74e, (7, valid.len() as u32, valid));
            }))
            .is_ok()
        );
        for bad in [
            make(0, 0x01030000, 0, 4),
            make(15, 0x01030000, 0, 4),
            make(1, 0x02000000, 0, 4),
            make(1, 0x01030000, 4, 4),
            make(1, 0x01030000, 0, 8),
            make(1, 0x01030000, 0, 0),
        ] {
            assert!(
                run(fixture(false, |f| {
                    f.insert(0xc74e, (7, bad.len() as u32, bad));
                }))
                .is_err()
            );
        }
        let valid = make(1, 0x01030000, 0, 4);
        for tag in [0xc740, 0xc741] {
            assert!(
                run(fixture(false, |f| {
                    f.insert(tag, (7, valid.len() as u32, valid.clone()));
                }))
                .is_err()
            );
        }
    }
}
