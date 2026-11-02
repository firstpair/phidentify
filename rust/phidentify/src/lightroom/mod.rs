//! Lightroom Classic's original-file fingerprint (the `AgLibraryFileDigest.digest`
//! value) for supported DNG strata, from the recreated engine
//! `native-original-digest-v5` (FirstPair fingerprint worker, 2026-10-05).
//!
//! Supported: integer, single-plane CFA or LinearRaw, lossless classic-TIFF DNG
//! with the opcode subset in [`opcodes`]. Anything else is an error, never a
//! guessed value. A match is native-matcher correspondence, not byte or pixel
//! equality; JPEG and non-DNG raw routes are not implemented here.
pub mod dng_metadata;
pub mod opcodes;
// The reviewed decoder is kept byte-for-byte; these lints are scoped to it.
#[allow(dead_code, clippy::needless_borrow)]
pub mod pixel_digest;
pub mod wrapper;

use crate::file::{self, Stamp};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const ENGINE_REVISION: &str = "native-original-digest-v5";

/// The camera and crop context Lightroom wraps around the raw-sample digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapperInputs {
    pub model: String,
    pub crop_size: [(u32, u32); 2],
    pub crop_origin: [(u32, u32); 2],
    pub user_crop: Option<[(u32, u32); 4]>,
    pub opcode_fingerprint_suffix: Vec<u8>,
}

impl WrapperInputs {
    pub fn from_metadata(m: &dng_metadata::DngMetadata) -> Self {
        Self {
            model: m.model.clone(),
            crop_size: m.crop_size,
            crop_origin: m.crop_origin,
            user_crop: m.user_crop,
            opcode_fingerprint_suffix: m.opcode_fingerprint_suffix.clone(),
        }
    }
    pub fn sha256(&self) -> Result<String> {
        let mut h = Sha256::new();
        h.update(b"LRCAT-V5-WRAPPER-INPUTS-1\0");
        h.update(serde_json::to_vec(self)?);
        Ok(file::hex(&h.finalize()))
    }
    /// Upper-case hex, as Classic stores it.
    pub fn fingerprint(&self, raw_digest: &[u8; 16]) -> Result<String> {
        let d = wrapper::wrap(raw_digest, &self.model, self.crop_size, self.crop_origin, self.user_crop,
                              &self.opcode_fingerprint_suffix)?;
        Ok(file::hex(&d).to_ascii_uppercase())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    pub engine_revision: String,
    /// Compare with `AgLibraryFileDigest.digest` (and `fileSize`).
    pub lightroom_original_digest: String,
    /// Adobe NewRawImageDigest recomputed from decoded samples (upper-case hex).
    pub adobe_new_raw_image_digest: String,
    pub wrapper_inputs_sha256: String,
}

/// Compute the fingerprint. Fails if the file changes while it is read.
pub fn compute(path: &Path) -> Result<Fingerprint> {
    let before: Stamp = file::stamp(path)?;
    let metadata = dng_metadata::read(path).context("supported DNG metadata")?;
    let pixels = pixel_digest::compute(path, metadata.linearization_count, metadata.raw_ifd_offset)?;
    ensure!(pixels.width == metadata.sensor_width && pixels.height == metadata.sensor_height,
            "decoded dimensions differ from raw IFD metadata");
    let wrapper = WrapperInputs::from_metadata(&metadata);
    let digest = wrapper.fingerprint(&pixels.digest)?;
    ensure!(file::stamp(path)? == before, "source changed during computation");
    Ok(Fingerprint {
        engine_revision: ENGINE_REVISION.into(),
        lightroom_original_digest: digest,
        adobe_new_raw_image_digest: file::hex(&pixels.digest).to_ascii_uppercase(),
        wrapper_inputs_sha256: wrapper.sha256()?,
    })
}
