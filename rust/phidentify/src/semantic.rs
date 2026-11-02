//! The DNG semantic identity: the strictest tiffgraph grammar that accepts a
//! DNG (Semantic Unison's `dng-xmp/0`, `/1`, `/2`), and its primary XMP packet
//! as the editable region for [`crate::file::preserved_sha256`]. Same order and
//! `max_xmp` as phidentify's OCaml package, so both record the same profile.
use tiffgraph::{Observers, Policy, SliceReader, WalkError, walk};

pub const MAX_XMP: usize = 1 << 20;

/// `(profile, regions)` for the strictest accepting grammar, or the reason the
/// last (most permissive) grammar refused the file.
pub fn strictest(bytes: &[u8]) -> Result<(String, Vec<(u64, u64)>), String> {
    let grammars = [("dng-xmp/0", Policy::v0(MAX_XMP)), ("dng-xmp/1", Policy::v1(MAX_XMP)),
                    ("dng-xmp/2", Policy::merge_only(MAX_XMP))];
    let mut reason = String::new();
    for (name, policy) in grammars {
        match walk(&policy, &mut SliceReader(bytes), Observers::default()) {
            Ok(Some(x)) => return Ok((name.to_string(), vec![(x.offset as u64, x.length as u64)])),
            Ok(None) => reason = "no XMP packet".to_string(),
            Err(WalkError::Refused(r)) => reason = r,
            Err(WalkError::Reader(r)) => reason = r,
        }
    }
    Err(reason)
}
