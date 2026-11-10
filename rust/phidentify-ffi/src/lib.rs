//! C ABI over phidentify. One call returns every requested identity as
//! NUL-separated fields, each `ok:<value>`, `err:<reason>` or empty when not
//! attempted, in this order: sha256, dng raw blocks, lightroom digest,
//! metadata digest, preserved (complement) digest, metadata record JSON, image
//! data digest, block marks, skeleton record JSON, packed skeleton in hex.
//! With `FLAG_COMPACT_RECORD` the record is [`phidentify::metadata::compact`]ed.
use phidentify::{Options, Outcome, identify};

// The macOS system allocator fragments under exiftool-rs's allocation pattern and
// keeps the memory: about 0.5 MB per extracted file, unbounded over a long run.
// mimalloc returns it (a 20-file loop stays flat), so the library uses it for all
// Rust allocations. OCaml's own heap is unaffected.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::{panic, ptr, slice};

pub const FLAG_DNG_BLOCKS: u32 = 1;
pub const FLAG_LIGHTROOM: u32 = 2;
pub const FLAG_METADATA: u32 = 4;
pub const FLAG_COMPACT_RECORD: u32 = 8;
pub const FLAG_IMAGE_DATA: u32 = 16;
pub const FLAG_BLOCK_MARKS: u32 = 32;
pub const FLAG_SKELETON: u32 = 64;

fn field(o: &Outcome) -> String {
    match o {
        None => String::new(),
        Some(Ok(v)) => format!("ok:{v}"),
        Some(Err(e)) => format!("err:{}", e.replace('\0', " ")),
    }
}

/// Fields for `path` under `flags`; `Err` when the file itself cannot be identified.
pub fn identify_fields(path: &Path, flags: u32, regions: Option<Vec<(u64, u64)>>) -> Result<String, String> {
    let opts = Options {
        dng_blocks: flags & FLAG_DNG_BLOCKS != 0,
        lightroom: flags & FLAG_LIGHTROOM != 0,
        metadata: flags & FLAG_METADATA != 0,
        preserved_regions: regions,
        dng_semantic: false,
        image_data: flags & FLAG_IMAGE_DATA != 0,
        block_marks: flags & FLAG_BLOCK_MARKS != 0,
        skeleton: flags & FLAG_SKELETON != 0,
    };
    let id = identify(path, opts).map_err(|e| format!("{e:#}"))?;
    let record = match &id.metadata_json {
        None => String::new(),
        Some(j) if flags & FLAG_COMPACT_RECORD != 0 => compact_field(j),
        Some(j) => format!("ok:{}", j.replace('\0', " ")),
    };
    Ok([format!("ok:{}", id.sha256), field(&id.dng_raw_blocks_sha256), field(&id.lightroom_digest),
        field(&id.metadata_digest), field(&id.preserved_sha256), record, field(&id.image_data_sha256),
        field(&id.block_marks_sha256), stored(&id.skeleton, &id.sha256)].join("\0"))
}

/// The two skeleton fields: its record and its packed bytes in hex.
fn stored(skeleton: &Option<Result<phidentify::skeleton::Skeleton, String>>, sha256: &str) -> String {
    match skeleton {
        None => "\0".to_string(),
        Some(Err(e)) => { let e = format!("err:{}", e.replace('\0', " ")); format!("{e}\0{e}") }
        Some(Ok(s)) => match s.pack() {
            Ok(packed) => format!("ok:{}\0ok:{}", s.record(sha256), phidentify::file::hex(&packed)),
            Err(e) => format!("err:{e}\0err:{e}"),
        },
    }
}

/// Block marks and the skeleton of `path`, whose content has this SHA-256,
/// from the file's structures alone (no whole read): three fields, block
/// marks, skeleton record, packed skeleton in hex.
pub fn skeleton_fields(path: &Path, sha256: &str) -> String {
    let built = phidentify::skeleton::build(path);
    let marks = Some(built.as_ref().map_err(String::clone).and_then(phidentify::skeleton::block_marks));
    [field(&marks), stored(&Some(built), sha256)].join("\0")
}

#[cfg(feature = "metadata")]
fn compact_field(json: &str) -> String {
    match phidentify::metadata::compact(json) {
        Ok(c) => format!("ok:{}", c.replace('\0', " ")),
        Err(e) => format!("err:{e}"),
    }
}
#[cfg(not(feature = "metadata"))]
fn compact_field(_: &str) -> String {
    String::new()
}

fn give(s: String, out: *mut *mut u8, out_len: *mut usize) {
    let boxed = s.into_bytes().into_boxed_slice();
    let len = boxed.len();
    // SAFETY: callers pass valid out pointers (checked non-null below).
    unsafe {
        *out_len = len;
        *out = Box::into_raw(boxed) as *mut u8;
    }
}

/// Status 0: `out` holds the fields. 1: `out` holds the reason the file could
/// not be identified. 2: invalid arguments. 3: panic. Free `out` with
/// `phidentify_free`. `regions` holds `n_regions` (offset, length) pairs, and
/// is null when no complement digest is wanted.
///
/// # Safety
/// `path` must point to `path_len` readable bytes, `regions` to `2 * n_regions`
/// u64 values when non-null; `out`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_identify(path: *const u8, path_len: usize, flags: u32, regions: *const u64,
                                             n_regions: usize, out: *mut *mut u8, out_len: *mut usize) -> i32 {
    if path.is_null() || out.is_null() || out_len.is_null() {
        return 2;
    }
    // SAFETY: guaranteed by the caller.
    let bytes = unsafe { slice::from_raw_parts(path, path_len) };
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    let regions = (!regions.is_null()).then(|| {
        // SAFETY: guaranteed by the caller.
        let r = unsafe { slice::from_raw_parts(regions, 2 * n_regions) };
        r.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect::<Vec<_>>()
    });
    match panic::catch_unwind(|| identify_fields(path, flags, regions)) {
        Ok(Ok(s)) => { give(s, out, out_len); 0 }
        Ok(Err(e)) => { give(e, out, out_len); 1 }
        Err(_) => { give("panic in phidentify".into(), out, out_len); 3 }
    }
}

/// Status 0: `out` holds [`skeleton_fields`]. 2: invalid arguments. 3: panic.
///
/// # Safety
/// `path` and `sha256` must point to their lengths of readable bytes;
/// `out`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_skeleton(path: *const u8, path_len: usize, sha256: *const u8, sha256_len: usize,
                                             out: *mut *mut u8, out_len: *mut usize) -> i32 {
    if path.is_null() || sha256.is_null() || out.is_null() || out_len.is_null() {
        return 2;
    }
    // SAFETY: guaranteed by the caller.
    let (path, sha) = unsafe { (slice::from_raw_parts(path, path_len), slice::from_raw_parts(sha256, sha256_len)) };
    let path = Path::new(std::ffi::OsStr::from_bytes(path));
    let sha = String::from_utf8_lossy(sha).into_owned();
    match panic::catch_unwind(|| skeleton_fields(path, &sha)) {
        Ok(s) => { give(s, out, out_len); 0 }
        Err(_) => { give("panic in phidentify".into(), out, out_len); 3 }
    }
}

/// 1 when the metadata identity applies to `path` (the engine reads its
/// extension; `metadata::reads`), else 0; 2 for invalid arguments. Without the
/// `metadata` feature, 0. No I/O.
///
/// # Safety
/// `path` must point to `path_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_reads_metadata(path: *const u8, path_len: usize) -> i32 {
    if path.is_null() {
        return 2;
    }
    // SAFETY: guaranteed by the caller.
    let bytes = unsafe { slice::from_raw_parts(path, path_len) };
    #[cfg(feature = "metadata")]
    return i32::from(phidentify::metadata::reads(Path::new(std::ffi::OsStr::from_bytes(bytes))));
    #[cfg(not(feature = "metadata"))]
    return { let _ = bytes; 0 };
}

/// # Safety
/// `out`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_version(out: *mut *mut u8, out_len: *mut usize) -> i32 {
    if out.is_null() || out_len.is_null() {
        return 2;
    }
    let mut v = format!("phidentify {}", env!("CARGO_PKG_VERSION"));
    if cfg!(feature = "lightroom") { v.push_str(" +lightroom"); }
    if cfg!(feature = "metadata") { v.push_str(" +metadata"); }
    give(v, out, out_len);
    0
}

/// # Safety
/// `ptr`/`len` must come from this library, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        // SAFETY: reconstructs the boxed slice handed out by `give`.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, len)) });
    }
}
