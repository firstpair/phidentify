---
title: "Which \"same\"? Named identities for a photo archive"
date: 2026-11-02
author: "Alexy Khrabrov"
slug: "phidentify"
canonical_url: "https://firstpair.press/phidentify/"
tags:
  - photography
  - dng
  - lightroom
  - rust
  - ocaml
  - oxcaml
  - open-source
---

![Two Victorian detectives study fingerprints on cased portrait photographs with a magnifying glass.](../../../cover/phidentify-headboard.png)

Every photo archive eventually asks the same small question: is this the same photo as that one? It sounds like a yes-or-no question. It is really five questions wearing one word, and an archive gets into trouble when it answers one of them and acts as if it had answered another.

[phidentify](https://github.com/firstpair/phidentify) is our answer to that. It computes several identities for a photo file, gives each one a name, and refuses to let any of them stand in for another. It is a Rust library with a C ABI and an OCaml package that builds with both stock OCaml and OxCaml. The [manual](https://firstpair.org/books/phidentify/) is now a preview on the First Pair Photography and Unix shelves.

## Five questions behind one word

Take a DNG and the copy of it in last year's backup. Their checksums differ. What does that mean?

Perhaps a rating changed. Lightroom can write metadata straight into a DNG, so a single star rewrites the XMP packet inside the file and changes the whole-file hash. The photograph is untouched. Perhaps the embedded preview was refreshed after an edit. Perhaps a converter moved the image data further into the file to make room for a larger packet. Or perhaps one byte of raw data was damaged on a failing drive, and the photograph really is different.

A whole-file checksum cannot tell these apart. It was never meant to. It answers "are these the same bytes?" and nothing else, and it answers that perfectly. The mistake is to ask it the other questions.

So phidentify asks them separately:

- **Whole-file SHA-256**: are the bytes identical?
- **DNG encoded raw blocks**: is the stored raw data the same, whatever happened to the metadata around it?
- **Lightroom Classic original fingerprint**: would Lightroom Classic accept this file as the original its catalog recorded?
- **Metadata record**: is the metadata the same, and if not, which fields changed?
- **Preserved digest**: is everything except the XMP packet identical?

Each has a name in every interface, and each reports one of three things: a value, a failure with a reason, or "not attempted". The last two are never a match. Two files that both failed to parse have not agreed on anything.

## The raw data, without the wrapper

The raw-block identity came from [dng-compare](https://github.com/firstpair/dng-compare), our tool for comparing DNG archives. Its `dng-subset` scanner pairs files and checks whether their encoded raw blocks match, block by block, without decoding. phidentify turns that comparison into a digest: the byte order, then for each primary raw image the encoding tags, the block counts and the block bytes, under a fixed domain string. Two DNGs get the same digest exactly when dng-subset would call them equal.

What it leaves out is the point. Offsets are not hashed, so a file whose image data moved when its metadata grew keeps its identity. XMP, previews and maker notes are not hashed. A keyword edit cannot change it. A flipped raw byte always does.

It is not a pixel identity. The same samples re-encoded with different compression get a different digest. Deciding that question needs decoding, and dng-compare does that when it has to.

## Lightroom's own idea of "the same"

Lightroom Classic keeps a fingerprint of every original in its catalog. When files move, or a library is restored, that value is how Classic recognizes its originals. If you want to know whether a file on disk is the one a catalog entry means, you need that value, and only Lightroom used to be able to compute it.

phidentify reimplements it for the DNGs it supports: integer, single-plane CFA or LinearRaw classic-TIFF DNGs, uncompressed or with lossless JPEG compression, with the two raw-image opcodes it implements. It decodes the raw samples before linearization, computes Adobe's NewRawImageDigest over them, and wraps that with the camera model, the default and user crops and a fingerprint of the opcode lists, all with MD5, as Lightroom does. It contains no Adobe code. Any file outside those strata is an error, never a guess.

The only evidence that matters for an identity like this is agreement with the original. On 2026-11-01 we fingerprinted 60 randomly chosen DNGs from a Lightroom Classic 15 catalog, and all 60 matched the values Classic had stored. That is what the identity claims, and all it claims: an equal fingerprint means Classic's matcher would pair the files. It does not mean their bytes or their pixels are equal.

## Metadata as a record, not a byte range

The metadata identity uses dng-compare's in-process exiftool-rs engine, which writes an ExifTool-shaped JSON record for each file. phidentify hashes that record without the members that only say where the file sits, its name, folder and dates, so a moved file keeps its metadata identity.

It keeps the engine's own stamp in the hash. That looks fussy, and it is deliberate. A different engine, or a newer version of the same one, can read the same file slightly differently. If records from two engines could compare equal, an archive would one day conclude that metadata had changed when only the reader had. With the stamp in the hash, records from different engines never compare at all.

For the question people usually ask next, "what changed?", there is a field-by-field diff. For storage, there is a compact form that replaces long strings such as base64 thumbnails with their SHA-256 and length.

## Everything except the packet

The last identity is the one a synchronizer needs. [Semantic Unison](https://github.com/querygraph/semantic-unison), our fork of the Unison file synchronizer, sends DNG metadata edits without resending image bytes. Before it does, it has to prove that the receiver's file and the sender's differ only in their XMP packets. Its proof is a hash of every byte outside the packet, with the file size and the packet's position bound in. Unison calls it `SDE/0-preserved`.

Finding the packet safely is the hard part, and it is the job of [tiffgraph](https://github.com/firstpair/tiffgraph), our pure-OCaml TIFF/DNG graph walker. Under a fixed grammar it locates the primary packet and checks that nothing else the file declares lies inside it. Unison's peers negotiate which grammar they trust, from the strict `dng-xmp/0` to `dng-xmp/2`, which also admits the private fields camera converters write.

phidentify walks each DNG with the strictest grammar that accepts it, records which one, and computes the same preserved digest. We cross-checked it byte for byte against Unison's own implementation on 60 DNGs: every file both accepted got the same grammar and the same digest, and the files without a packet were refused by both. Two files with equal preserved digests, under the same grammar, can differ only inside the packet. That makes it the precise answer to "was this only a metadata edit?"

## Parallel, in two kinds of OCaml

The OCaml package identifies many files at once with a pool of domains. Under stock OCaml 5 it uses `Domain.spawn`. Under OxCaml it uses `Domain.Safe.spawn`, which accepts only portable closures, so the workers share nothing but an immutable list of paths and an atomic counter. The tiffgraph walk runs inside each worker, over a file descriptor the worker opened itself, and the Rust core runs with the OCaml runtime lock released. dune picks the right pool for the switch, and the test suite runs in both.

## What the names buy

With the identities named, the archive questions get short answers. Is this file in my backup? Same SHA-256, yes; same raw blocks, the photograph is there but this version is not; same preserved digest, the backup plus this file's XMP packet restores it exactly. Did the copy that went out on a portable drive come back intact? Only SHA-256 answers that. Is this the original my catalog recorded? Only the Lightroom fingerprint answers that, and only for the files it supports.

None of those answers is hard. What is hard is keeping them apart, especially when one of them is missing. phidentify's rule is the one that makes an archive trustworthy: a failed or missing identity is not a match, and one identity never answers for another.

The [manual](https://firstpair.org/books/phidentify/) covers each identity in detail, with exact formulas, failure reporting and cost, the Rust, C and OCaml interfaces, and a worked guide to which identity answers which question. Its [PDF](https://firstpair.org/phidentify/pdf/), [EPUB](https://firstpair.org/phidentify/epub/) and [HTML](https://firstpair.org/read/phidentify/) editions are on the shelf, and the code is MIT-licensed at [github.com/firstpair/phidentify](https://github.com/firstpair/phidentify).

*Colophon: written by Alexy Khrabrov with Claude as co-author. The ideas are Alexy's; Claude wrote the text and the code with him.*
