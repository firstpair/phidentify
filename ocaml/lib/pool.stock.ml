(* Stock OCaml 5 pool: Domain.spawn workers pull indexes from an atomic counter
   and each returns its own (index, result) list, so no array is shared. *)
let backend = "domains-stock"
let recommended () = Domain.recommended_domain_count ()

external identify : string -> int -> int64 array option -> int * string = "phidentify_identify_stub"
external open_ro : string -> int * int64 = "phidentify_open_ro_stub"
external pread : int -> int64 -> int -> string = "phidentify_pread_stub"
external close : int -> unit = "phidentify_close_stub"

(* The strictest tiffgraph grammar that accepts the file, and its XMP packet. *)
let walk path =
  match open_ro path with
  | exception Failure m -> Error m
  | fd, size ->
    let reader = { Tiffgraph.size; read = (fun off len -> pread fd off len); check = (fun () -> ()) } in
    let attempt name policy =
      match Tiffgraph.walk policy reader with
      | Ok (Some (x : Tiffgraph.xmp)) -> Ok (name, x)
      | Ok None -> Error "no XMP packet"
      | Error e -> Error e
      | exception Failure m -> Error m
      | exception Invalid_argument m -> Error m in
    let max_xmp = 1 lsl 20 in
    let result =
      match attempt "dng-xmp/0" (Tiffgraph.v0 ~max_xmp) with
      | Ok _ as r -> r
      | Error _ ->
        match attempt "dng-xmp/1" (Tiffgraph.v1 ~max_xmp) with
        | Ok _ as r -> r
        | Error _ -> attempt "dng-xmp/2" (Tiffgraph.merge_only ~max_xmp) in
    close fd;
    result

(* One file: (identify status and payload, semantic profile outcome). *)
let one ~flags ~semantic path =
  let sem = if semantic then Some (walk path) else None in
  let regions = match sem with
    | Some (Ok (_, (x : Tiffgraph.xmp))) -> Some [| x.offset; Int64.of_int x.length |]
    | _ -> None in
  match identify path flags regions with
  | raw -> Ok (raw, sem)
  | exception Failure m -> Error m

let run ~workers ~flags ~semantic (paths : string array) =
  let next = Atomic.make 0 in
  let work () =
    let rec loop acc =
      let i = Atomic.fetch_and_add next 1 in
      if i < Array.length paths then loop ((i, one ~flags ~semantic paths.(i)) :: acc) else acc in
    loop [] in
  List.init workers (fun _ -> Domain.spawn work) |> List.concat_map Domain.join
