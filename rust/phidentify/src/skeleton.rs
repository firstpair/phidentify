//! Skeletons and the block-marks identity.
//!
//! A metadata edit (a rating, develop settings, IPTC) rewrites a file's small
//! structures and carries its large blocks over unchanged, perhaps to other
//! offsets. A file is cut by its own structure into **blocks** and the
//! **skeleton**: every byte outside the blocks, in file order. The cutting
//! rules are those of dng-compare's archive format (its specification,
//! section 13.1), so a skeleton made here is the one `dngc` makes.
//!
//! - [`build`] gives a file's skeleton and block map. It reads the file's
//!   structures, the bytes outside its blocks and each block's two ends, never
//!   a whole block.
//! - [`rebuild`] writes the file as it was, from its skeleton and a later
//!   version of the file that still holds the blocks. The caller compares the
//!   SHA-256 it returns with the content's: a mark covers only a block's ends.
//! - [`block_marks`] is an identity: equal values mean the two files hold the
//!   same blocks, by length and mark, wherever they sit. It covers what the
//!   raw-block and image-data identities leave out (embedded previews, maker
//!   notes, profiles). It is a sampled identity, a strong hint and not proof:
//!   a mark hashes a block's first and last 4,096 bytes only.
//!
//! Walkers: classic TIFF (DNG and TIFF-based raws) and JPEG. A file of another
//! format no larger than [`WHOLE`] is its own skeleton, with no blocks (a
//! sidecar); a larger one has no skeleton.
use crate::file::hex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// An out-of-line TIFF value or JPEG segment this large is a block.
pub const BLOCK: u64 = 512;
/// A run this long that no tag points at is a block.
pub const GAP: u64 = 65536;
/// A block's mark hashes this many bytes from each end (the whole block when
/// it is no longer than twice this).
pub const MARK: u64 = 4096;
/// Strips or tiles of one image this close together are one block.
pub const PAD: u64 = 3;
/// A file without a walker is stored whole up to this size.
pub const WHOLE: u64 = 1 << 20;
/// The zstd level skeletons are packed at. A reader accepts any level.
pub const LEVEL: i32 = 19;

/// The reason given for a file no walker reads.
pub const NO_WALKER: &str = "no skeleton walker for this format";
/// The reason [`block_marks`] gives for a file that has a skeleton and no blocks.
pub const NO_BLOCKS: &str = "no blocks";

const DOMAIN: &[u8] = b"PHIDENTIFY-BLOCK-MARKS-1\0";
/// XMP, IPTC and Photoshop resources: what editors rewrite, kept in the skeleton.
const VOLATILE: [u16; 3] = [700, 33723, 34377];
/// SubIFDs, Exif, GPS and Interoperability.
const POINTERS: [u16; 4] = [330, 34665, 34853, 40965];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub at: u64,
    pub len: u64,
    /// First 32 hex digits of the SHA-256 of the block's two ends.
    pub mark: String,
    pub role: String,
}

/// A file's skeleton: its format (`tiff`, `jpeg` or `whole`), size, blocks in
/// file order, and every byte outside them, concatenated in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    pub format: String,
    pub size: u64,
    pub blocks: Vec<Block>,
    pub bytes: Vec<u8>,
}

/// Bytes read by position: a file, or a file already in memory.
pub trait Source {
    fn size(&self) -> u64;
    /// Up to `len` bytes at `at`; fewer at the end.
    fn read_at(&mut self, at: u64, len: u64) -> Result<Vec<u8>, String>;
}

pub struct Bytes<'a>(pub &'a [u8]);

impl Source for Bytes<'_> {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&mut self, at: u64, len: u64) -> Result<Vec<u8>, String> {
        let size = self.0.len() as u64;
        let from = at.min(size);
        let to = at.saturating_add(len).min(size);
        Ok(self.0[from as usize..to as usize].to_vec())
    }
}

pub struct OnDisk {
    file: File,
    size: u64,
}

impl OnDisk {
    pub fn open(path: &Path) -> Result<OnDisk, String> {
        let file = File::open(path).map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        Ok(OnDisk { file, size })
    }
}

impl Source for OnDisk {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&mut self, at: u64, len: u64) -> Result<Vec<u8>, String> {
        self.file.seek(SeekFrom::Start(at)).map_err(io)?;
        let mut out = Vec::new();
        (&mut self.file).take(len).read_to_end(&mut out).map_err(io)?;
        Ok(out)
    }
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn rd16(le: bool, b: &[u8]) -> u64 {
    (if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }) as u64
}

fn rd32(le: bool, b: &[u8]) -> u64 {
    (if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }) as u64
}

struct Entry {
    tag: u16,
    typ: u16,
    raw: [u8; 4],
    total: u64,
}

fn type_size(typ: u16) -> u64 {
    match typ {
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

/// An entry's unsigned integer values (BYTE, SHORT, LONG, IFD).
fn values(f: &mut dyn Source, le: bool, e: &Entry) -> Vec<u64> {
    let width = match e.typ {
        1 => 1,
        3 => 2,
        4 | 13 => 4,
        _ => return Vec::new(),
    };
    let buf = if e.total <= 4 {
        e.raw[..e.total as usize].to_vec()
    } else {
        let at = rd32(le, &e.raw);
        if at + e.total > f.size() {
            return Vec::new();
        }
        f.read_at(at, e.total).unwrap_or_default()
    };
    if buf.len() as u64 != e.total {
        return Vec::new();
    }
    buf.chunks(width).map(|c| match width {
        1 => c[0] as u64,
        2 => rd16(le, c),
        _ => rd32(le, c),
    }).collect()
}

type Extent = (u64, u64, String);

fn tiff(f: &mut dyn Source) -> Result<Vec<Extent>, String> {
    let size = f.size();
    let h = f.read_at(0, 8)?;
    let le = match h.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err("not a TIFF".into()),
    };
    if h.len() < 8 || rd16(le, &h[2..4]) != 42 {
        return Err("not a classic TIFF".into());
    }
    let mut blocks: Vec<Extent> = Vec::new();
    let mut claims: Vec<(u64, u64)> = vec![(0, 8)];
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([(rd32(le, &h[4..8]), "0".to_string())]);
    while let Some((mut off, role)) = queue.pop_front() {
        let mut n = 0;
        while off != 0 && off + 6 <= size && seen.insert(off) {
            let head = f.read_at(off, 2)?;
            if head.len() < 2 {
                break;
            }
            let cnt = rd16(le, &head);
            let data = f.read_at(off + 2, cnt * 12 + 4)?;
            if (data.len() as u64) < cnt * 12 + 4 {
                break;
            }
            let r = if n == 0 { role.clone() } else { format!("{role}+{n}") };
            claims.push((off, 6 + cnt * 12));
            let entries: Vec<Entry> = data[..(cnt * 12) as usize].chunks(12).map(|c| {
                let typ = rd16(le, &c[2..4]) as u16;
                Entry { tag: rd16(le, &c[0..2]) as u16, typ, raw: [c[8], c[9], c[10], c[11]], total: type_size(typ) * rd32(le, &c[4..8]) }
            }).collect();
            let find = |tag: u16| entries.iter().find(|e| e.tag == tag);
            for (otag, ctag, name) in [(273, 279, "strips"), (324, 325, "tiles")] {
                if let (Some(o), Some(c)) = (find(otag), find(ctag)) {
                    let (os, cs) = (values(f, le, o), values(f, le, c));
                    for (o, c) in os.into_iter().zip(cs) {
                        if c > 0 && o + c <= size {
                            blocks.push((o, c, format!("{r}/{name}")));
                        }
                    }
                }
            }
            if let (Some(o), Some(c)) = (find(513), find(514))
                && let (Some(&o), Some(&c)) = (values(f, le, o).first(), values(f, le, c).first())
                && c > 0 && o + c <= size
            {
                blocks.push((o, c, format!("{r}/jpeg")));
            }
            for e in &entries {
                if e.total <= 4 {
                    continue;
                }
                let at = rd32(le, &e.raw);
                if at + e.total > size {
                    continue;
                }
                claims.push((at, e.total));
                if e.total >= BLOCK && !VOLATILE.contains(&e.tag) && !POINTERS.contains(&e.tag) && ![273, 279, 324, 325].contains(&e.tag) {
                    blocks.push((at, e.total, format!("{r}/tag{}", e.tag)));
                }
            }
            for tag in POINTERS {
                if let Some(e) = find(tag) {
                    for (k, o) in values(f, le, e).into_iter().enumerate() {
                        queue.push_back((o, format!("{r}/{tag}.{k}")));
                    }
                }
            }
            off = rd32(le, &data[(cnt * 12) as usize..]);
            n += 1;
        }
    }
    // Data only a maker's own structures reach: no tag points at it, and an
    // edit carries it over as it is.
    let mut all: Vec<(u64, u64)> = claims.into_iter().chain(blocks.iter().map(|b| (b.0, b.1))).collect();
    all.push((size, 0));
    all.sort();
    let mut pos = 0;
    for (o, l) in all {
        if o > pos && o - pos >= GAP {
            blocks.push((pos, o - pos, "gap".into()));
        }
        pos = pos.max(o + l);
    }
    Ok(blocks)
}

fn jpeg(f: &mut dyn Source) -> Result<Vec<Extent>, String> {
    let size = f.size();
    let mut blocks = Vec::new();
    let mut pos = 2;
    while pos + 4 <= size {
        let m = f.read_at(pos, 4)?;
        if m.len() < 4 || m[0] != 0xff {
            break;
        }
        let marker = m[1];
        if marker == 0xff {
            pos += 1;
            continue;
        }
        // Start of scan, or a marker without a length: the image runs from here.
        if marker == 0xda || marker == 0x00 || marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
            break;
        }
        let seg = rd16(false, &m[2..4]) + 2;
        if seg < 4 || pos + seg > size {
            break;
        }
        // Exif and XMP (APP1), IPTC (APP13) and comments stay in the skeleton.
        if ![0xe1, 0xed, 0xfe].contains(&marker) && seg >= BLOCK {
            blocks.push((pos, seg, format!("seg{marker:02x}")));
        }
        pos += seg;
    }
    if pos < size {
        blocks.push((pos, size - pos, "scan".into()));
    }
    Ok(blocks)
}

/// Sorted, without overlaps; the strips or tiles of one image merged when no
/// more than PAD bytes apart.
fn normalize(mut blocks: Vec<Extent>) -> Vec<Extent> {
    blocks.sort();
    blocks.dedup();
    let mut out: Vec<Extent> = Vec::new();
    for (o, l, role) in blocks {
        if let Some(last) = out.last_mut() {
            let end = last.0 + last.1;
            if o < end {
                continue;
            }
            if role == last.2 && (role.ends_with("strips") || role.ends_with("tiles")) && o - end <= PAD {
                last.1 = o + l - last.0;
                continue;
            }
        }
        out.push((o, l, role));
    }
    out
}

/// A file's format and blocks, decided by its first bytes.
fn walk(f: &mut dyn Source) -> Result<(&'static str, Vec<Extent>), String> {
    let magic = f.read_at(0, 4)?;
    match magic.as_slice() {
        [b'I', b'I', 42, 0] | [b'M', b'M', 0, 42] => Ok(("tiff", normalize(tiff(f)?))),
        [0xff, 0xd8, ..] => Ok(("jpeg", normalize(jpeg(f)?))),
        _ => Err(NO_WALKER.into()),
    }
}

fn mark(f: &mut dyn Source, at: u64, len: u64) -> Result<String, String> {
    let mut h = Sha256::new();
    if len <= 2 * MARK {
        h.update(f.read_at(at, len)?);
    } else {
        h.update(f.read_at(at, MARK)?);
        h.update(f.read_at(at + len - MARK, MARK)?);
    }
    Ok(hex(&h.finalize()[..16]))
}

/// The skeleton of what `source` holds.
pub fn build_from(f: &mut dyn Source) -> Result<Skeleton, String> {
    let size = f.size();
    let (format, extents) = match walk(f) {
        Ok(w) => w,
        Err(_) if size <= WHOLE => ("whole", Vec::new()),
        Err(e) => return Err(e),
    };
    let (mut blocks, mut bytes, mut pos) = (Vec::new(), Vec::new(), 0);
    for (at, len, role) in extents {
        bytes.extend(f.read_at(pos, at - pos)?);
        blocks.push(Block { at, len, mark: mark(f, at, len)?, role });
        pos = at + len;
    }
    bytes.extend(f.read_at(pos, size - pos)?);
    let outside = size - blocks.iter().map(|b| b.len).sum::<u64>();
    if bytes.len() as u64 != outside {
        return Err("file changed while its skeleton was read".into());
    }
    Ok(Skeleton { format: format.into(), size, blocks, bytes })
}

/// The skeleton of the file at `path`.
pub fn build(path: &Path) -> Result<Skeleton, String> {
    build_from(&mut OnDisk::open(path)?)
}

/// The skeleton of a file held in memory.
pub fn build_bytes(bytes: &[u8]) -> Result<Skeleton, String> {
    build_from(&mut Bytes(bytes))
}

/// Rebuild the skeleton's content into `sink` with blocks taken from `edited`,
/// a later version of the same file; returns the SHA-256 of what was written.
/// The caller compares it with the content's SHA-256.
pub fn rebuild_from(skeleton: &Skeleton, edited: &mut dyn Source, sink: &mut dyn Write) -> Result<String, String> {
    let mut h = Sha256::new();
    let mut put = |d: &[u8]| -> Result<(), String> {
        h.update(d);
        sink.write_all(d).map_err(io)
    };
    if skeleton.blocks.is_empty() {
        if skeleton.bytes.len() as u64 != skeleton.size {
            return Err("skeleton shorter than its content".into());
        }
        put(&skeleton.bytes)?;
        return Ok(hex(&h.finalize()));
    }
    let (_, extents) = walk(edited)?;
    let mut found: HashMap<(u64, String), u64> = HashMap::new();
    for (at, len, _) in extents {
        found.entry((len, mark(edited, at, len)?)).or_insert(at);
    }
    let (mut pos, mut skel) = (0u64, 0usize);
    for b in &skeleton.blocks {
        let n = b.at.checked_sub(pos).ok_or("blocks out of order in a skeleton")? as usize;
        put(skeleton.bytes.get(skel..skel + n).ok_or("skeleton shorter than its map")?)?;
        skel += n;
        let at = *found.get(&(b.len, b.mark.clone()))
            .ok_or_else(|| format!("block {} ({} bytes) is not in the edited file", b.role, b.len))?;
        let mut done = 0;
        while done < b.len {
            let want = (b.len - done).min(1 << 20);
            let piece = edited.read_at(at + done, want)?;
            if piece.len() as u64 != want {
                return Err("edited file changed while it was read".into());
            }
            put(&piece)?;
            done += want;
        }
        pos = b.at + b.len;
    }
    let rest = skeleton.bytes.get(skel..).ok_or("skeleton shorter than its map")?;
    if pos + rest.len() as u64 != skeleton.size {
        return Err("skeleton does not fit its map".into());
    }
    put(rest)?;
    Ok(hex(&h.finalize()))
}

/// [`rebuild_from`] with the edited file at `edited`.
pub fn rebuild(skeleton: &Skeleton, edited: &Path, sink: &mut dyn Write) -> Result<String, String> {
    rebuild_from(skeleton, &mut OnDisk::open(edited)?, sink)
}

/// The block-marks identity of a skeleton: SHA-256 over the domain string, the
/// format, the number of blocks and each block's length and mark, sorted by
/// length then mark. Offsets and roles are left out, so blocks an edit moved
/// keep it. A file stored whole, or with no blocks, has none.
pub fn block_marks(skeleton: &Skeleton) -> Result<String, String> {
    if skeleton.blocks.is_empty() {
        return Err(if skeleton.format == "whole" { NO_WALKER } else { NO_BLOCKS }.into());
    }
    let mut pairs: Vec<(u64, &str)> = skeleton.blocks.iter().map(|b| (b.len, b.mark.as_str())).collect();
    pairs.sort();
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(skeleton.format.as_bytes());
    h.update([0]);
    h.update((pairs.len() as u64).to_le_bytes());
    for (len, mark) in pairs {
        h.update(len.to_le_bytes());
        h.update(mark.as_bytes());
    }
    Ok(hex(&h.finalize()))
}

fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Skeleton {
    /// The skeleton's bytes as one zstd frame.
    pub fn pack(&self) -> Result<Vec<u8>, String> {
        zstd::encode_all(self.bytes.as_slice(), LEVEL).map_err(io)
    }

    /// The record stored beside a packed skeleton, for the content with this
    /// SHA-256: JSON as Python's `json.dumps` writes it, with the members of
    /// dng-compare's skeleton record that do not depend on where the file is:
    /// `sha256`, `size`, `format`, `blocks` (`[at, len, mark, role]` each),
    /// `skeleton_bytes`, `skeleton_sha256`, `skeleton_format`.
    pub fn record(&self, sha256: &str) -> String {
        let blocks: Vec<String> = self.blocks.iter()
            .map(|b| format!("[{}, {}, {}, {}]", b.at, b.len, quoted(&b.mark), quoted(&b.role))).collect();
        format!("{{\"sha256\": {}, \"size\": {}, \"format\": {}, \"blocks\": [{}], \"skeleton_bytes\": {}, \
                 \"skeleton_sha256\": {}, \"skeleton_format\": \"zstd\"}}",
                quoted(sha256), self.size, quoted(&self.format), blocks.join(", "), self.bytes.len(),
                quoted(&crate::file::sha256_bytes(&self.bytes)))
    }

    /// A skeleton from its record and packed bytes, checked against the
    /// record's `skeleton_bytes` and `skeleton_sha256` and for blocks in order.
    pub fn from_record(record: &str, packed: &[u8]) -> Result<Skeleton, String> {
        let j: serde_json::Value = serde_json::from_str(record).map_err(|e| format!("skeleton record: {e}"))?;
        let n = |v: &serde_json::Value| v.as_u64().ok_or("bad number in a skeleton record");
        let text = |v: &serde_json::Value| v.as_str().map(str::to_string).ok_or("bad string in a skeleton record");
        let bytes = zstd::decode_all(packed).map_err(|e| format!("skeleton bytes: {e}"))?;
        if bytes.len() as u64 != n(&j["skeleton_bytes"])? || crate::file::sha256_bytes(&bytes) != text(&j["skeleton_sha256"])? {
            return Err("skeleton bytes do not match their record".into());
        }
        let size = n(&j["size"])?;
        let mut blocks = Vec::new();
        let mut pos = 0;
        for b in j["blocks"].as_array().ok_or("no blocks in a skeleton record")? {
            let a = b.as_array().filter(|a| a.len() == 4).ok_or("bad block in a skeleton record")?;
            let block = Block { at: n(&a[0])?, len: n(&a[1])?, mark: text(&a[2])?, role: text(&a[3])? };
            if block.at < pos || block.at.checked_add(block.len).is_none_or(|end| end > size) {
                return Err("blocks out of order in a skeleton record".into());
            }
            pos = block.at + block.len;
            blocks.push(block);
        }
        Ok(Skeleton { format: text(&j["format"])?, size, blocks, bytes })
    }
}
