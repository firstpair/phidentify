//! Metadata records from dng-compare's exiftool-rs engine (`dng-exiftool-ffi`).
//!
//! A record is one ExifTool-shaped JSON object (`-G0:1:4 -n -struct`, duplicates
//! kept) carrying `ExifTool:DngCompareEngine`. Records from different engines
//! are separate identities: the digest includes the engine stamp, so they never
//! compare equal. A failed extraction is an `ExifTool:Error` member, reported
//! here as an error, never as empty metadata.
use anyhow::{Result, bail};
use serde_json::{Map, Value};
use std::path::Path;

pub use dng_exiftool_ffi::{ERROR_TAG, engine_version};

/// Members that describe where and when the file sits, not what it contains.
pub fn is_volatile(key: &str) -> bool {
    key == "SourceFile" || key.starts_with("File:System:")
}

#[derive(Debug, Clone)]
pub struct Record {
    /// The engine's record, verbatim.
    pub json: String,
    /// SHA-256 of the canonical (sorted-key) record without volatile members.
    pub digest: String,
}

pub fn extract(path: &Path) -> Result<Record> {
    let json = dng_exiftool_ffi::record_json(path);
    let value: Value = serde_json::from_str(&json)?;
    let Value::Object(map) = value else { bail!("engine record is not an object") };
    if let Some(error) = map.get(ERROR_TAG) {
        bail!("metadata extraction failed: {error}");
    }
    let kept: Map<String, Value> = map.into_iter().filter(|(k, _)| !is_volatile(k)).collect();
    let canonical = serde_json::to_vec(&Value::Object(kept))?;
    Ok(Record { json, digest: crate::file::sha256_bytes(&canonical) })
}

/// The record with every string member over 1 KiB (embedded thumbnails as
/// `base64:`, camera-profile tables) replaced by `long-sha256:<hex>:<length>`,
/// for storage. Digests are always taken over the full record.
pub fn compact(json: &str) -> Result<String> {
    fn walk(v: &mut Value) {
        match v {
            Value::String(s) if s.len() > 1024 => {
                *s = format!("long-sha256:{}:{}", crate::file::sha256_bytes(s.as_bytes()), s.len());
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            Value::Object(o) => o.values_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v: Value = serde_json::from_str(json)?;
    walk(&mut v);
    Ok(serde_json::to_string(&v)?)
}

/// One differing member: (key, left value, right value), `None` when absent on that side.
pub type Difference = (String, Option<Value>, Option<Value>);

/// Members whose values differ between two records, ignoring volatile ones.
pub fn diff(left: &str, right: &str) -> Result<Vec<Difference>> {
    let parse = |s: &str| -> Result<Map<String, Value>> {
        match serde_json::from_str(s)? {
            Value::Object(m) => Ok(m),
            _ => bail!("record is not an object"),
        }
    };
    let (l, r) = (parse(left)?, parse(right)?);
    let mut keys: Vec<&String> = l.keys().chain(r.keys()).filter(|k| !is_volatile(k)).collect();
    keys.sort();
    keys.dedup();
    Ok(keys
        .into_iter()
        .filter(|k| l.get(*k) != r.get(*k))
        .map(|k| (k.clone(), l.get(k).cloned(), r.get(k).cloned()))
        .collect())
}
