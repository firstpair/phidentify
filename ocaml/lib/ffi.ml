(* Direct bindings to the phidentify-ffi C ABI. *)
external version : unit -> string = "phidentify_version_stub"
external identify : string -> int -> int * string = "phidentify_identify_stub"
