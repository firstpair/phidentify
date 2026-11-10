type outcome = Not_attempted | Ok of string | Failed of string

type stored = { record : string; packed : string }

type identity = {
  sha256 : string;
  dng_raw_blocks : outcome;
  lightroom : outcome;
  metadata : outcome;
  dng_semantic : outcome;
  preserved : outcome;
  metadata_record : string option;
  image_data : outcome;
  block_marks : outcome;
  skeleton : (stored, string) result option;
}

type request = { dng_raw_blocks : bool; lightroom : bool; metadata : bool;
                 dng_semantic : bool; compact_record : bool; image_data : bool;
                 block_marks : bool; skeleton : bool }

let all = { dng_raw_blocks = true; lightroom = true; metadata = true; dng_semantic = true; compact_record = false;
            image_data = true; block_marks = true; skeleton = false }
let image_data_not_applicable = "not a TIFF-family file or JPEG"
let image_data_dng = "DNG: the raw-block identity applies instead"
let image_data_prefix_len = 16
let no_skeleton_walker = "no skeleton walker for this format"

let image_data_candidate p =
  let starts s = String.starts_with ~prefix:s p in
  starts "\xff\xd8\xff" || starts "FUJIFILMCCD-RAW" || (String.length p >= 4 && (starts "II" || starts "MM"))

let version = Ffi.version
let reads_metadata = Ffi.reads_metadata
let backend = Pool.backend
let recommended_workers = Pool.recommended

let flags (r : request) =
  (if r.dng_raw_blocks then 1 else 0) lor (if r.lightroom then 2 else 0) lor (if r.metadata then 4 else 0)
  lor (if r.compact_record then 8 else 0) lor (if r.image_data then 16 else 0)
  lor (if r.block_marks then 32 else 0) lor (if r.skeleton then 64 else 0)

let prefixed p s = String.length s >= String.length p && String.sub s 0 (String.length p) = p
let rest p s = String.sub s (String.length p) (String.length s - String.length p)

let outcome = function
  | "" -> Not_attempted
  | s when prefixed "ok:" s -> Ok (rest "ok:" s)
  | s when prefixed "err:" s -> Failed (rest "err:" s)
  | s -> Failed ("malformed field: " ^ s)

let unhex s =
  let digit c = match c with
    | '0' .. '9' -> Char.code c - 48 | 'a' .. 'f' -> Char.code c - 87 | _ -> invalid_arg "hex" in
  String.init (String.length s / 2) (fun i -> Char.chr ((digit s.[2 * i] lsl 4) lor digit s.[(2 * i) + 1]))

(* The skeleton record and packed-bytes fields. *)
let stored record packed =
  match outcome record, outcome packed with
  | Not_attempted, _ -> None
  | Ok record, Ok hex -> (match unhex hex with
    | packed -> Some (Result.Ok { record; packed })
    | exception Invalid_argument _ -> Some (Error "malformed packed skeleton"))
  | Failed reason, _ | _, Failed reason -> Some (Error reason)
  | Ok _, Not_attempted -> Some (Error "missing packed skeleton")

let skeleton ~sha256 path =
  match Ffi.skeleton path sha256 with
  | exception Failure m -> Failed m, Some (Error m)
  | _, payload ->
    match String.split_on_char '\000' payload with
    | [ marks; record; packed ] -> outcome marks, stored record packed
    | _ -> Failed "malformed phidentify fields", Some (Error "malformed phidentify fields")

let decode ((status, payload), sem) : (identity, string) result =
  if status <> 0 then Error payload
  else match String.split_on_char '\000' payload with
    | [ sha; blocks; lr; md; preserved; record; image; marks; skeleton_record; packed ] ->
      let dng_semantic = match sem with
        | None -> Not_attempted
        | Some (Result.Ok (profile, _)) -> Ok profile
        | Some (Error reason) -> Failed reason in
      (match outcome sha with
       | Ok sha256 ->
         Result.Ok { sha256; dng_raw_blocks = outcome blocks; lightroom = outcome lr; metadata = outcome md;
                     dng_semantic; preserved = outcome preserved;
                     metadata_record = (match outcome record with Ok j -> Some j | _ -> None);
                     image_data = outcome image; block_marks = outcome marks;
                     skeleton = stored skeleton_record packed }
       | _ -> Error "missing sha256")
    | _ -> Error "malformed phidentify fields"

let identify ?(request = all) path =
  match Pool.one ~flags:(flags request) ~semantic:request.dng_semantic path with
  | Result.Ok raw -> decode raw
  | Error m -> Error m

let identify_all ?(request = all) ~workers paths =
  if workers < 1 then invalid_arg "workers must be positive";
  let paths = Array.of_list paths in
  let n = Array.length paths in
  if n = 0 then [] else begin
    let workers = min workers (min n (max 1 (Pool.recommended ()))) in
    let results = Array.make n None in
    List.iter (fun (i, r) -> results.(i) <- Some r)
      (Pool.run ~workers ~flags:(flags request) ~semantic:request.dng_semantic paths);
    Array.to_list (Array.mapi (fun i slot -> match slot with
      | Some (Result.Ok raw) -> decode raw
      | Some (Error message) -> Error message
      | None -> Error ("missing result for " ^ paths.(i))) results)
  end
