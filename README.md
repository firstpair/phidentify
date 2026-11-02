# phidentify

Named, separate identities for photo files, as a Rust library with an OCaml
and OxCaml binding.

| Identity | Equal values mean | They do not mean |
| --- | --- | --- |
| `sha256` | every byte is equal | — |
| `dng_raw_blocks` | same encoded primary raw data in a DNG, with the same encoding tags | equal metadata, XMP or previews; equal pixels after a re-encode |
| `lightroom` | Lightroom Classic's original matcher pairs the files (`AgLibraryFileDigest.digest`) | byte or pixel equality |
| `metadata` | equal exiftool-rs metadata records, with file location and dates left out | equal image data |
| `preserved` (with `dng_semantic`) | every byte outside the DNG's primary XMP packet is equal, at the same size and layout | anything about the XMP packet |
| `image_data` | same stored image bytes in a camera raw, TIFF or JPEG that is not a DNG | equal metadata; equal pixels after a re-encode |

One identity never stands in for another. An identity that was not attempted,
failed, or does not apply to the file is reported as such, never as a match.

## Where the identities come from

- **DNG raw blocks** follow the block comparison in
  [dng-compare](https://github.com/firstpair/dng-compare)'s `dng-subset`
  scanner. Two DNGs get the same digest exactly when that comparison finds
  them equal: byte order, number of primary raw images, and for each image its
  encoding-tag signature, block counts and block bytes. Metadata edits written
  into the DNG, such as keywords or ratings, leave it unchanged.
- **Image data** is the same kind of identity for files that are not DNGs. It
  hashes only the stored image bytes: the strips and tiles of every IFD in a
  TIFF or TIFF-based raw (NEF, CR2, ARW, 3FR, PEF, ORF and others), the raw data
  of a Panasonic RW2 (RawDataOffset to the end of the file), the CFA section of
  a Fujifilm RAF, and a JPEG's frame, tables and scans without its APPn and COM
  segments. A capture-time shift or other metadata edit leaves it unchanged; on
  real TIFF, ARW, RW2 and JPEG files shifted with ExifTool, the whole-file
  SHA-256 changed and the image data did not. DNGs are refused (their raw
  blocks are the identity that ignores regenerated previews), as are CR3, HEIC
  and other containers.
- **Lightroom fingerprint** is a reimplementation of the value Lightroom
  Classic stores per original file. It covers integer, single-plane CFA or
  LinearRaw, lossless classic-TIFF DNGs, with the FixVignetteRadial and
  FixBadPixelsConstant opcodes. Any other file is an error, never a guessed
  value. Raw samples are decoded with [rawler](https://crates.io/crates/rawler).
  Qualification: on 2026-11-01, 60 randomly chosen DNGs from a Lightroom
  Classic 15 catalog all matched the fingerprints Classic had stored for them.
  JPEG and camera-raw routes are not implemented.
- **Metadata** uses dng-compare's exiftool-rs engine (`dng-exiftool-ffi`).
  Records carry `ExifTool:DngCompareEngine`, and the digest includes it, so
  records from different engines never compare equal. They are a separate
  identity from Perl ExifTool output. `metadata::diff` lists the members that
  differ between two records.

- **Preserved (complement) digest** is Semantic Unison's `SDE/0-preserved`
  hash: SHA-256 over every byte outside the editable regions, with the file
  size and region geometry bound in. The OCaml package finds the region with
  [tiffgraph](https://github.com/firstpair/tiffgraph), using the strictest of
  Unison's grammars `dng-xmp/0`, `/1` and `/2` that accepts the file, and
  records which one. Rust callers pass regions in `Options::preserved_regions`.

## Rust

```sh
cd rust
cargo build --release                         # sha256, DNG raw blocks, Lightroom
cargo build --release --features phidentify/metadata
cargo test --release
```

```rust
use phidentify::{identify, Options};
let id = identify(path, Options { dng_blocks: true, lightroom: true, metadata: false, preserved_regions: None })?;
println!("{} {:?} {:?}", id.sha256, id.dng_raw_blocks_sha256, id.lightroom_digest);
```

`identify` reads files up to 1 GiB once into memory and takes every identity
from the same bytes. Larger files get a streamed SHA-256 only. It fails if the
file changes while it is read.

`cargo run --release --example identify -- FILE...` prints one JSON line per file.

## OCaml and OxCaml

The `ocaml/` dune project (opam package `phidentify`) binds the static library
built from `rust/phidentify-ffi`. It builds with stock OCaml 5, using a
`Domain.spawn` pool, and with OxCaml, using `Domain.Safe.spawn` with portable
closures. The switch decides which pool is compiled in.

```sh
(cd rust && cargo build --release -p phidentify-ffi)
cd ocaml
opam exec --switch 5.2.0+ox -- dune build && opam exec --switch 5.2.0+ox -- dune test --force
```

`PHIDENTIFY_FFI_LIBDIR` overrides where the Rust archive is found (default
`../rust/target/release`). `PHIDENTIFY_FFI_LINK=dynamic` links the shared
library instead.

```ocaml
match Phidentify.identify path with
| Ok id -> print_endline id.sha256
| Error reason -> prerr_endline reason
```

`Phidentify.identify_all ~workers paths` returns results in input order. The
`phidentify [-j N] FILE...` executable prints one tab-separated line per file.

## TIFF graph walker

The bounded TIFF/DNG walker that locates a DNG's editable XMP packet, shared
with Semantic Unison, lives in its own repository, `tiffgraph` (opam
`tiffgraph`, pure OCaml for stock OCaml and OxCaml). It briefly lived here as
`ocaml/tiff`.

## Manual

The manual, [docs/phidentify-manual.md](docs/phidentify-manual.md), describes each
identity in detail: inputs, what equality proves and does not, reporting, cost, and
which identity answers which question. It is on the First Pair Photography and Unix shelves at
[firstpair.org/books/phidentify](https://firstpair.org/books/phidentify/);
`FIRSTPAIR.md` records how it is built and published.

## Licences

The phidentify source is MIT. Builds link rawler (LGPL-2.1) for the Lightroom
fingerprint. The `metadata` feature links exiftool-rs (GPL-3.0-or-later): a
binary built with it is covered by the GPL. See dng-compare's
`rust-scanner/exiftool-ffi/THIRD-PARTY.md`.
