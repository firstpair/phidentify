(** Separate, named identities for photo files, over the phidentify Rust
    library. None substitutes for another; [Not_attempted] and [Failed] are
    never matches.

    - [sha256]: every byte.
    - [dng_raw_blocks]: encoded primary raw data of a DNG (dng-compare
      semantics); ignores metadata, XMP and block placement.
    - [lightroom]: Lightroom Classic's original fingerprint
      ([AgLibraryFileDigest.digest]) for supported DNG strata; native-matcher
      correspondence, not byte or pixel equality.
    - [metadata]: digest of the exiftool-rs record without location members,
      plus the record; a separate identity from Perl ExifTool output.
    - [dng_semantic] and [preserved]: the strictest tiffgraph grammar
      (["dng-xmp/0"], ["/1"], ["/2"], Semantic Unison's profiles) that accepts
      the DNG, and Unison's [SDE/0-preserved] digest of every byte outside its
      primary XMP packet, with size and layout bound in. Equal [preserved]
      means only the XMP packet can differ.
    - [image_data]: the stored image bytes of a camera raw, TIFF or JPEG that
      is not a DNG (strips and tiles; RW2 raw data; RAF CFA; JPEG frame,
      tables and scans without APPn/COM). A metadata edit such as a
      capture-time shift keeps it; re-encoding does not. *)

type outcome = Not_attempted | Ok of string | Failed of string

type identity = {
  sha256 : string;
  dng_raw_blocks : outcome;
  lightroom : outcome;
  metadata : outcome;
  dng_semantic : outcome;  (** [Ok profile] *)
  preserved : outcome;
  metadata_record : string option;
  (** exiftool-rs JSON, when requested; with [compact_record], string members
      over 1 KiB are replaced by [long-sha256:<hex>:<length>]. *)
  image_data : outcome;
}

type request = { dng_raw_blocks : bool; lightroom : bool; metadata : bool;
                 dng_semantic : bool; compact_record : bool; image_data : bool }

val all : request
(** Every identity the linked library supports. *)

val image_data_candidate : string -> bool
(** Whether a file starting with these bytes (its first [image_data_prefix_len],
    or all of a shorter file) can have [image_data]; when false, its
    [image_data] fails with [image_data_not_applicable]. Same test as the Rust
    [image_data::candidate]. *)

val image_data_prefix_len : int
val image_data_not_applicable : string
val image_data_dng : string
(** The reasons [image_data] gives for a non-candidate and for a DNG. *)

val version : unit -> string
(** e.g. ["phidentify 0.2.0 +lightroom +metadata"]. *)

val backend : string
(** ["domains-stock"] (OCaml 5 [Domain.spawn]) or ["domains-oxcaml"]
    (OxCaml [Domain.Safe.spawn] with portable closures). *)

val recommended_workers : unit -> int

val identify : ?request:request -> string -> (identity, string) result
(** [Error reason] when the file cannot be read or changed while read. *)

val identify_all : ?request:request -> workers:int -> string list -> (identity, string) result list
(** Results in input order, computed by [min workers paths] domains, capped
    at [recommended_workers ()]. *)
