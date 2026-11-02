//! The DNG semantic identity: the strictest tiffgraph grammar that accepts a
//! DNG (Semantic Unison's `dng-xmp/0`, `/1`, `/2`), and its primary XMP packet
//! as the editable region for [`crate::file::preserved_sha256`]. Same order and
//! `max_xmp` as phidentify's OCaml package, so both record the same profile.
use tiffgraph::{FileReader, Observers, Policy, Reader, SliceReader, WalkError, walk};

pub const MAX_XMP: usize = 1 << 20;

/// `(profile, regions)` for the strictest accepting grammar, or the reason the
/// last (most permissive) grammar refused the file.
pub fn strictest(bytes: &[u8]) -> Result<(String, Vec<(u64, u64)>), String> {
    strictest_with(|| Ok(SliceReader(bytes)), |e| e)
}

/// The same, reading the file at `path` positionally: for files kept out of
/// memory. Same reasons as [`strictest`], as phidentify's OCaml file reader gives.
pub fn strictest_file(path: &std::path::Path) -> Result<(String, Vec<(u64, u64)>), String> {
    strictest_with(|| FileReader::open(path).map_err(|e| e.to_string()), |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof { "read outside the bytes".to_string() } else { e.to_string() }
    })
}

fn strictest_with<R: Reader>(mut open: impl FnMut() -> Result<R, String>, reason_of: impl Fn(R::Error) -> String)
                             -> Result<(String, Vec<(u64, u64)>), String> {
    let grammars = [("dng-xmp/0", Policy::v0(MAX_XMP)), ("dng-xmp/1", Policy::v1(MAX_XMP)),
                    ("dng-xmp/2", Policy::merge_only(MAX_XMP))];
    let mut reason = String::new();
    for (name, policy) in grammars {
        match walk(&policy, &mut open()?, Observers::default()) {
            Ok(Some(x)) => return Ok((name.to_string(), vec![(x.offset as u64, x.length as u64)])),
            Ok(None) => reason = "no XMP packet".to_string(),
            Err(WalkError::Refused(r)) => reason = r,
            Err(WalkError::Reader(r)) => reason = reason_of(r),
        }
    }
    Err(reason)
}
