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
      plus the record; a separate identity from Perl ExifTool output. *)

type outcome = Not_attempted | Ok of string | Failed of string

type identity = {
  sha256 : string;
  dng_raw_blocks : outcome;
  lightroom : outcome;
  metadata : outcome;
  metadata_record : string option;  (** exiftool-rs JSON, when requested *)
}

type request = { dng_raw_blocks : bool; lightroom : bool; metadata : bool }

val all : request
(** Every identity the linked library supports. *)

val version : unit -> string
(** e.g. ["phidentify 0.1.0 +lightroom +metadata"]. *)

val backend : string
(** ["domains-stock"] (OCaml 5 [Domain.spawn]) or ["domains-oxcaml"]
    (OxCaml [Domain.Safe.spawn] with portable closures). *)

val recommended_workers : unit -> int

val identify : ?request:request -> string -> (identity, string) result
(** [Error reason] when the file cannot be read or changed while read. *)

val identify_all : ?request:request -> workers:int -> string list -> (identity, string) result list
(** Results in input order, computed by [min workers paths] domains, capped
    at [recommended_workers ()]. *)
