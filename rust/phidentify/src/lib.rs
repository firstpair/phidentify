//! Named, separate identities for photo files.
//!
//! | Identity | Proves | Does not prove |
//! |---|---|---|
//! | [`file`] SHA-256 | every byte equal | — |
//! | [`dng_blocks`] encoded raw blocks | same encoded primary raw data (dng-compare semantics) | equal metadata, decoded pixels across re-encodes |
//! | [`image_data`] image bytes of non-DNG raws, TIFFs, JPEGs | same stored image bytes, metadata aside | equal metadata, decoded pixels across re-encodes |
//! | [`lightroom`] original fingerprint | Lightroom's original matcher would pair them | byte or pixel equality |
//! | [`metadata`] record digest | equal exiftool-rs metadata, location excluded | equal image data |
//! | [`file::preserved_sha256`] over caller regions | every byte outside the regions equal, same size and layout | anything about the regions' contents |
//!
//! None of these substitutes for another; a missing or failed identity is not a match.
pub mod dng_blocks;
pub mod file;
pub mod image_data;
#[cfg(feature = "lightroom")]
pub mod lightroom;
#[cfg(feature = "metadata")]
pub mod metadata;
#[cfg(feature = "semantic")]
pub mod semantic;

use anyhow::{Result, ensure};
use serde::Serialize;
use std::path::Path;

/// Files up to this size are read once into memory and every identity is taken
/// from the same bytes; larger files get a streamed SHA-256 only.
pub const IN_MEMORY_LIMIT: u64 = 1 << 30;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub dng_blocks: bool,
    pub lightroom: bool,
    pub metadata: bool,
    /// Editable regions found by a tiffgraph walk; when given, the
    /// `SDE/0-preserved` complement digest is computed over the same bytes.
    pub preserved_regions: Option<Vec<(u64, u64)>>,
    /// Walk the in-memory bytes with tiffgraph (strictest of dng-xmp/0, /1, /2)
    /// and take `preserved_regions` from it; needs the `semantic` feature.
    pub dng_semantic: bool,
    /// Image-data identity of TIFF-family raws, TIFFs and JPEGs (not DNGs).
    pub image_data: bool,
}

/// An identity that was not attempted is `None`; one that failed or does not
/// apply is `Some(Err(reason))`.
pub type Outcome = Option<std::result::Result<String, String>>;

#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub stamp: file::Stamp,
    pub sha256: String,
    pub dng_raw_blocks_sha256: Outcome,
    pub lightroom_digest: Outcome,
    pub metadata_digest: Outcome,
    pub preserved_sha256: Outcome,
    /// `Ok(profile)` of the strictest tiffgraph grammar that accepts the file.
    pub dng_semantic_profile: Outcome,
    pub image_data_sha256: Outcome,
    #[serde(skip)]
    pub metadata_json: Option<String>,
}

fn is_tiff(b: &[u8]) -> bool {
    b.len() >= 4 && (b[..4] == *b"II*\0" || b[..4] == *b"MM\0*" || b[..4] == *b"II+\0" || b[..4] == *b"MM\0+")
}

/// Compute the requested identities. Fails, with nothing returned, if the file
/// cannot be read or changes while it is being read.
pub fn identify(path: &Path, opts: Options) -> Result<Identity> {
    let before = file::stamp(path)?;
    let (sha256, bytes) = if before.size <= IN_MEMORY_LIMIT {
        let bytes = std::fs::read(path)?;
        ensure!(bytes.len() as u64 == before.size, "size changed while reading");
        (file::sha256_bytes(&bytes), Some(bytes))
    } else {
        (file::sha256_file(path)?, None)
    };
    let tiff = bytes.as_deref().filter(|b| is_tiff(b));
    let dng = if opts.dng_blocks || opts.lightroom { tiff.map(dng_blocks::parse) } else { None };
    let is_dng = matches!(dng, Some(Ok(_)));
    let dng_raw_blocks_sha256 = match (&dng, opts.dng_blocks) {
        (Some(Ok(d)), true) => Some(dng_blocks::raw_blocks_sha256(tiff.unwrap(), d).map_err(|e| e.to_string())),
        (Some(Err(e)), true) => Some(Err(e.to_string())),
        _ => None,
    };
    #[cfg(feature = "lightroom")]
    let lightroom_digest = (opts.lightroom && is_dng)
        .then(|| lightroom::compute(path).map(|f| f.lightroom_original_digest).map_err(|e| format!("{e:#}")));
    #[cfg(not(feature = "lightroom"))]
    let lightroom_digest = (opts.lightroom && is_dng).then(|| Err("built without the lightroom feature".to_string()));
    #[cfg(feature = "metadata")]
    let (metadata_digest, metadata_json) = if opts.metadata {
        match metadata::extract(path) {
            Ok(r) => (Some(Ok(r.digest)), Some(r.json)),
            Err(e) => (Some(Err(e.to_string())), None),
        }
    } else {
        (None, None)
    };
    #[cfg(not(feature = "metadata"))]
    let (metadata_digest, metadata_json) =
        (opts.metadata.then(|| Err("built without the metadata feature".to_string())), None);
    #[cfg(feature = "semantic")]
    let walked = opts.dng_semantic.then(|| match bytes.as_deref() {
        Some(b) => semantic::strictest(b),
        None => semantic::strictest_file(path),
    });
    #[cfg(not(feature = "semantic"))]
    let walked: Option<std::result::Result<(String, Vec<(u64, u64)>), String>> =
        opts.dng_semantic.then(|| Err("built without the semantic feature".to_string()));
    let regions = match &walked {
        Some(Ok((_, r))) => Some(r.clone()),
        Some(Err(_)) => None,
        None => opts.preserved_regions.clone(),
    };
    let dng_semantic_profile = walked.as_ref().map(|w| w.as_ref().map(|(p, _)| p.clone()).map_err(|e| e.clone()));
    let preserved_sha256 = regions.as_deref().map(|regions| match bytes.as_deref() {
        Some(b) => file::preserved_sha256(b, regions).map_err(|e| e.to_string()),
        None => Err("file over the in-memory limit".to_string()),
    });
    let image_data_sha256 = opts.image_data.then(|| match bytes.as_deref() {
        Some(b) => image_data::sha256(b).map_err(|e| e.to_string()),
        None => Err("file over the in-memory limit".to_string()),
    });
    ensure!(file::stamp(path)? == before, "file changed while being identified");
    Ok(Identity { stamp: before, sha256, dng_raw_blocks_sha256, lightroom_digest, metadata_digest, preserved_sha256,
                  dng_semantic_profile, image_data_sha256, metadata_json })
}
