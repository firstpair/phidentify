//! C ABI over phidentify. One call returns every requested identity as
//! NUL-separated fields, each `ok:<value>`, `err:<reason>` or empty when not
//! attempted, in this order: sha256, dng raw blocks, lightroom digest,
//! metadata digest, metadata record JSON.
use phidentify::{Options, Outcome, identify};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::{panic, ptr, slice};

pub const FLAG_DNG_BLOCKS: u32 = 1;
pub const FLAG_LIGHTROOM: u32 = 2;
pub const FLAG_METADATA: u32 = 4;

fn field(o: &Outcome) -> String {
    match o {
        None => String::new(),
        Some(Ok(v)) => format!("ok:{v}"),
        Some(Err(e)) => format!("err:{}", e.replace('\0', " ")),
    }
}

/// Fields for `path` under `flags`; `Err` when the file itself cannot be identified.
pub fn identify_fields(path: &Path, flags: u32) -> Result<String, String> {
    let opts = Options {
        dng_blocks: flags & FLAG_DNG_BLOCKS != 0,
        lightroom: flags & FLAG_LIGHTROOM != 0,
        metadata: flags & FLAG_METADATA != 0,
    };
    let id = identify(path, opts).map_err(|e| format!("{e:#}"))?;
    let json = id.metadata_json.as_ref().map(|j| format!("ok:{}", j.replace('\0', " "))).unwrap_or_default();
    Ok([format!("ok:{}", id.sha256), field(&id.dng_raw_blocks_sha256), field(&id.lightroom_digest),
        field(&id.metadata_digest), json].join("\0"))
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
/// `phidentify_free`.
///
/// # Safety
/// `path` must point to `path_len` readable bytes; `out`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phidentify_identify(path: *const u8, path_len: usize, flags: u32,
                                                  out: *mut *mut u8, out_len: *mut usize) -> i32 {
    if path.is_null() || out.is_null() || out_len.is_null() {
        return 2;
    }
    // SAFETY: guaranteed by the caller.
    let bytes = unsafe { slice::from_raw_parts(path, path_len) };
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    match panic::catch_unwind(|| identify_fields(path, flags)) {
        Ok(Ok(s)) => { give(s, out, out_len); 0 }
        Ok(Err(e)) => { give(e, out, out_len); 1 }
        Err(_) => { give("panic in phidentify".into(), out, out_len); 3 }
    }
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
