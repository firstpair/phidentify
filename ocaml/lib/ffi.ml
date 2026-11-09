(* Direct bindings to the phidentify-ffi C ABI and the file-read stubs. *)
external version : unit -> string = "phidentify_version_stub"
external reads_metadata : string -> bool = "phidentify_reads_metadata_stub" [@@noalloc]
external identify : string -> int -> int64 array option -> int * string = "phidentify_identify_stub"
