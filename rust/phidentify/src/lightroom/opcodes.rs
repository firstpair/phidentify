//! Verified DNG opcode fingerprint serialization. No file I/O or decoding.
//!
//! Adobe's on-disk OpcodeList is big-endian. FingerprintToStream writes its
//! typed values into the caller's stream; installed Lightroom 15.6 uses a
//! little-endian stream. Only FixVignetteRadial (3) and FixBadPixelsConstant (4)
//! are implemented here. Canonical integration matches seven Lightroom-created
//! reference fingerprints and preserves all 100 earlier DNG reference matches.
//!
//! Lightroom fingerprints the crop, then the optional user crop, then opcode
//! lists 1, 2 and 2 again; list 3 is not part of the fingerprint.

use anyhow::{Context, Result, bail, ensure};

const MAX_OPCODES: usize = 4096;
const MAX_TAG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    FixVignetteRadial {
        coefficients: [f64; 5],
        center_h: f64,
        center_v: f64,
    },
    FixBadPixelsConstant {
        constant: u32,
        bayer_phase: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opcode {
    pub minimum_version: u32,
    pub flags: u32,
    pub payload: Payload,
}

impl Opcode {
    fn validate(&self) -> Result<()> {
        let version = self.minimum_version.to_be_bytes();
        ensure!(
            version[0] == 1 && (3..=7).contains(&version[1]),
            "unverified opcode minimum version"
        );
        ensure!(self.flags & !3 == 0, "unverified opcode flags");
        match &self.payload {
            Payload::FixVignetteRadial {
                coefficients,
                center_h,
                center_v,
            } => {
                ensure!(
                    coefficients.iter().all(|value| value.is_finite())
                        && center_h.is_finite()
                        && center_v.is_finite(),
                    "nonfinite vignette parameters"
                );
                ensure!(
                    (0.0..=1.0).contains(center_h) && (0.0..=1.0).contains(center_v),
                    "vignette center outside0..1"
                );
            }
            Payload::FixBadPixelsConstant { bayer_phase, .. } => {
                ensure!(*bayer_phase <= 3, "invalid Bayer phase");
            }
        }
        Ok(())
    }

    fn id_and_length(&self) -> (u32, u32) {
        match self.payload {
            Payload::FixVignetteRadial { .. } => (3, 56),
            Payload::FixBadPixelsConstant { .. } => (4, 8),
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .position
            .checked_add(N)
            .context("opcode offset overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .context("truncated opcode list")?;
        self.position = end;
        Ok(bytes.try_into()?)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn f64(&mut self) -> Result<f64> {
        // Reinterpret the original bits. Do not round, normalize signed zero,
        // or perform arithmetic before writing this value back to the stream.
        Ok(f64::from_bits(u64::from_be_bytes(self.take()?)))
    }
}

/// Parse a present raw-IFD OpcodeList tag. An absent tag is handled by the
/// suffix helper; a present zero-byte tag is malformed. Exactly four zero
/// bytes represent a valid empty list. Trailing bytes always fail closed.
pub fn parse_list(tag_bytes: &[u8]) -> Result<Vec<Opcode>> {
    ensure!(
        tag_bytes.len() <= MAX_TAG_BYTES,
        "opcode tag exceeds metadata limit"
    );
    let mut reader = Reader {
        bytes: tag_bytes,
        position: 0,
    };
    let count = usize::try_from(reader.u32()?)?;
    ensure!(count <= MAX_OPCODES, "too many opcodes");
    let mut list = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.u32()?;
        let minimum_version = reader.u32()?;
        let flags = reader.u32()?;
        let data_bytes = reader.u32()?;
        let payload = match id {
            3 => {
                ensure!(data_bytes == 56, "FixVignetteRadial payload must be56bytes");
                let mut coefficients = [0.0; 5];
                for coefficient in &mut coefficients {
                    *coefficient = reader.f64()?;
                }
                Payload::FixVignetteRadial {
                    coefficients,
                    center_h: reader.f64()?,
                    center_v: reader.f64()?,
                }
            }
            4 => {
                ensure!(
                    data_bytes == 8,
                    "FixBadPixelsConstant payload must be8bytes"
                );
                Payload::FixBadPixelsConstant {
                    constant: reader.u32()?,
                    bayer_phase: reader.u32()?,
                }
            }
            0 => bail!("private opcode is unsupported"),
            _ => bail!("unsupported opcode ID{id}"),
        };
        let opcode = Opcode {
            minimum_version,
            flags,
            payload,
        };
        opcode.validate()?;
        list.push(opcode);
    }
    ensure!(
        reader.position == tag_bytes.len(),
        "trailing bytes after opcode list"
    );
    Ok(list)
}

/// Reproduce Adobe FingerprintToStream for these two typed opcode families.
/// Empty lists write no count field and therefore contribute no bytes.
pub fn serialize_list(list: &[Opcode]) -> Result<Vec<u8>> {
    ensure!(list.len() <= MAX_OPCODES, "too many opcodes");
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::with_capacity(4 + list.len() * 76);
    bytes.extend(u32::try_from(list.len())?.to_le_bytes());
    for opcode in list {
        opcode.validate()?;
        let (id, length) = opcode.id_and_length();
        for field in [id, opcode.minimum_version, opcode.flags, length] {
            bytes.extend(field.to_le_bytes());
        }
        match &opcode.payload {
            Payload::FixVignetteRadial {
                coefficients,
                center_h,
                center_v,
            } => {
                for value in coefficients.iter().chain([center_h, center_v]) {
                    bytes.extend(value.to_bits().to_le_bytes());
                }
            }
            Payload::FixBadPixelsConstant {
                constant,
                bayer_phase,
            } => {
                bytes.extend(constant.to_le_bytes());
                bytes.extend(bayer_phase.to_le_bytes());
            }
        }
    }
    Ok(bytes)
}

pub fn fingerprint_stream(tag_bytes: &[u8]) -> Result<Vec<u8>> {
    serialize_list(&parse_list(tag_bytes)?)
}

/// Research suffix for the installed ordinary-DNG serializer. Append these
/// bytes after model, crop and optional user crop; do not hash a list3 tag here.
/// Callers must select the original raw IFD, never reduced preview IFDs.
/// This function returns bytes only and does not enable a canonical hash path.
pub fn installed_15_6_suffix(list1: Option<&[u8]>, list2: Option<&[u8]>) -> Result<Vec<u8>> {
    let one = list1
        .map(fingerprint_stream)
        .transpose()?
        .unwrap_or_default();
    let two = list2
        .map(fingerprint_stream)
        .transpose()?
        .unwrap_or_default();
    let mut result = Vec::with_capacity(one.len() + two.len() * 2);
    result.extend(&one);
    result.extend(&two);
    result.extend(&two);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Vec<u8> {
        assert!(value.len().is_multiple_of(2));
        (0..value.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
            .collect()
    }

    // Byte vectors independently prepared from the inspected original tag
    // parameters using Adobe's typed PutData definitions. They are serializer
    // oracles, not yet Lightroom final-fingerprint equality claims.
    const GOLDENS: [(&str, &str); 7] = [
        (
            "00000001000000030103000000000001000000383fb61677ad2e3267bfcc4f953af88c563fe3fd32d5bcadabbfe7f11b35f3a34e3fd467bfabd784163fe00000000000003fe0000000000000",
            "010000000300000000000301010000003800000067322ead7716b63f568cf83a954fccbfabadbcd532fde33f4ea3f3351bf1e7bf1684d7abbf67d43f000000000000e03f000000000000e03f",
        ),
        (
            "00000001000000040103000000000000000000080000000000000002",
            "01000000040000000000030100000000080000000000000002000000",
        ),
        (
            "00000001000000030103000000000001000000383fa5c6dda07454e83ff03cc47f15be57c003f6943bd955fd400572722dd46d36bfefc7c40e11755f3fe00000000000003fe0000000000000",
            "0100000003000000000003010100000038000000e85474a0ddc6a53f57be157fc43cf03ffd55d93b94f603c0366dd42d727205405f75110ec4c7efbf000000000000e03f000000000000e03f",
        ),
        (
            "00000001000000030103000000000001000000383fb51c9d114dc563bfe09430c46ec4133ff517d91451433abff6f0f2cc4b44893fe27d0db8b129c43fe00000000000003fe0000000000000",
            "010000000300000000000301010000003800000063c54d119d1cb53f13c46ec43094e0bf3a435114d917f53f89444bccf2f0f6bfc429b1b80d7de23f000000000000e03f000000000000e03f",
        ),
        (
            "00000001000000040103000000000000000000080000000000000000",
            "01000000040000000000030100000000080000000000000000000000",
        ),
        (
            "00000001000000030103000000000001000000383fd0b66960e765b03fd61f73933245043fb05f71c057aaf13fa2119f999d3805bf9f79cf7b5b2ba53fe00000000000003fe0000000000000",
            "0100000003000000000003010100000038000000b065e76069b6d03f04453293731fd63ff1aa57c0715fb03f05389d999f11a23fa52b5b7bcf799fbf000000000000e03f000000000000e03f",
        ),
        (
            "00000001000000030103000000000001000000383fd0a2001ec218a13fd5e98aefa1bf2a3fb023bdc41b8cba3fa1b9fb8d003c7abf9ebb6d657a7bf83fe00000000000003fe0000000000000",
            "0100000003000000000003010100000038000000a118c21e00a2d03f2abfa1ef8ae9d53fba8c1bc4bd23b03f7a3c008dfbb9a13ff87b7a656dbb9ebf000000000000e03f000000000000e03f",
        ),
    ];

    #[test]
    fn seven_independent_typed_byte_vectors() {
        for (source, expected) in GOLDENS {
            assert_eq!(fingerprint_stream(&hex(source)).unwrap(), hex(expected));
        }
    }

    #[test]
    fn absent_and_valid_empty_lists_write_nothing_but_malformed_empty_rejects() {
        assert_eq!(serialize_list(&[]).unwrap(), Vec::<u8>::new());
        assert_eq!(fingerprint_stream(&[0; 4]).unwrap(), Vec::<u8>::new());
        assert_eq!(installed_15_6_suffix(None, None).unwrap(), Vec::<u8>::new());
        assert_eq!(
            installed_15_6_suffix(Some(&[0; 4]), Some(&[0; 4])).unwrap(),
            Vec::<u8>::new()
        );
        for bytes in [&[][..], &[0][..], &[0; 3][..], &[0; 5][..]] {
            assert!(fingerprint_stream(bytes).is_err());
        }
    }

    #[test]
    fn suffix_keeps_list1_then_list2_twice_without_combining_counts() {
        let one = hex(GOLDENS[1].0);
        let two = hex(GOLDENS[0].0);
        let expected = [hex(GOLDENS[1].1), hex(GOLDENS[0].1), hex(GOLDENS[0].1)].concat();
        assert_eq!(
            installed_15_6_suffix(Some(&one), Some(&two)).unwrap(),
            expected
        );
        let mut multi = 2u32.to_be_bytes().to_vec();
        multi.extend(&one[4..]);
        multi.extend(&two[4..]);
        let mut expected_multi = 2u32.to_le_bytes().to_vec();
        expected_multi.extend(&hex(GOLDENS[1].1)[4..]);
        expected_multi.extend(&hex(GOLDENS[0].1)[4..]);
        assert_eq!(fingerprint_stream(&multi).unwrap(), expected_multi);
    }

    #[test]
    fn all_truncations_unknown_private_and_trailing_bytes_reject() {
        for (source, _) in GOLDENS {
            let source = hex(source);
            for end in 0..source.len() {
                assert!(fingerprint_stream(&source[..end]).is_err());
            }
            let mut extra = source;
            extra.push(0);
            assert!(fingerprint_stream(&extra).is_err());
        }
        for id in [0u32, 1, 2, 5, 8, 14, u32::MAX] {
            let mut source = hex(GOLDENS[1].0);
            source[4..8].copy_from_slice(&id.to_be_bytes());
            assert!(fingerprint_stream(&source).is_err());
        }
        let too_many = (MAX_OPCODES as u32 + 1).to_be_bytes();
        assert!(parse_list(&too_many).is_err());
        assert!(parse_list(&vec![0; MAX_TAG_BYTES + 1]).is_err());
    }

    #[test]
    fn preserves_versions_flags_and_finite_float_bit_patterns() {
        let mut raw = hex(GOLDENS[0].0);
        raw[8..12].copy_from_slice(&0x01070102u32.to_be_bytes());
        raw[12..16].copy_from_slice(&3u32.to_be_bytes());
        raw[20..28].copy_from_slice(&(-0.0f64).to_bits().to_be_bytes());
        let stream = fingerprint_stream(&raw).unwrap();
        assert_eq!(&stream[8..12], &0x01070102u32.to_le_bytes());
        assert_eq!(&stream[12..16], &3u32.to_le_bytes());
        assert_eq!(&stream[20..28], &(-0.0f64).to_bits().to_le_bytes());
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for position in [20usize, 60, 68] {
                let mut bad = raw.clone();
                bad[position..position + 8].copy_from_slice(&value.to_bits().to_be_bytes());
                assert!(fingerprint_stream(&bad).is_err());
            }
        }
    }

    #[test]
    fn rejects_invalid_lengths_versions_flags_centers_and_phase() {
        for length in [0u32, 7, 9, u32::MAX] {
            let mut raw = hex(GOLDENS[1].0);
            raw[16..20].copy_from_slice(&length.to_be_bytes());
            assert!(fingerprint_stream(&raw).is_err());
        }
        for version in [0u32, 0x01020000, 0x01080000, 0x02000000] {
            let mut raw = hex(GOLDENS[1].0);
            raw[8..12].copy_from_slice(&version.to_be_bytes());
            assert!(fingerprint_stream(&raw).is_err());
        }
        let mut raw = hex(GOLDENS[1].0);
        raw[12..16].copy_from_slice(&4u32.to_be_bytes());
        assert!(fingerprint_stream(&raw).is_err());
        let mut raw = hex(GOLDENS[1].0);
        raw[24..28].copy_from_slice(&4u32.to_be_bytes());
        assert!(fingerprint_stream(&raw).is_err());
        for center in [-0.01f64, 1.01] {
            let mut raw = hex(GOLDENS[0].0);
            raw[60..68].copy_from_slice(&center.to_bits().to_be_bytes());
            assert!(fingerprint_stream(&raw).is_err());
        }
        let invalid = Opcode {
            minimum_version: 0x01030000,
            flags: 0,
            payload: Payload::FixVignetteRadial {
                coefficients: [0.; 5],
                center_h: f64::NAN,
                center_v: 0.5,
            },
        };
        assert!(serialize_list(&[invalid]).is_err());
    }
}
