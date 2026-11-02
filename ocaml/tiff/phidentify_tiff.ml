(* Bounded TIFF/DNG graph walker: locates the primary XMP packet, protects
   every other declared extent, and reports physical regions. It is not a DNG
   validator or a raw-image fingerprint. Moved unchanged from Semantic Unison's
   DngSemantic; a caller that edits the XMP packet must still prove that every
   byte outside it is preserved. *)
type reader = { size : int64; read : int64 -> int -> string; check : unit -> unit }
type xmp = { offset : int64; length : int }
type policy = { extended : bool; opaque_maker : bool; layout_only : bool;
                require_xmp : bool; max_xmp : int }
exception Unsupported of string
let reject s = raise (Unsupported s)
type directory_kind = Image | Exif | Gps | Interop | CameraProfile

type tiff_layout = {
  big : bool; count_width : int; entry_size : int; number_width : int;
  inline_width : int; value_at : int;
}
let classic = {big=false;count_width=2;entry_size=12;number_width=4;
  inline_width=4;value_at=8}
let bigtiff = {big=true;count_width=8;entry_size=20;number_width=8;
  inline_width=8;value_at=12}
type directory = {
  kind : directory_kind; little : bool; layout : tiff_layout;
  base : int64; offset : int64;
}
type pending = Directory of directory | CameraHeader of int64

(* Values in this deliberately finite profile either live wholly in their
   TIFF value extent or use one of the graph/image-offset rules below.
   Unknown fields are not safe to treat as ordinary values: private LONGs
   and opaque MakerNotes can contain offsets into the proposed XMP packet. *)
let image_value_tag = function
  | 254 | 255 | 256 | 257 | 258 | 259 | 262 | 263 | 264 | 265 | 266
  | 269 | 270 | 271 | 272 | 274 | 277 | 278 | 280 | 281 | 282 | 283
  | 284 | 285 | 286 | 287 | 290 | 291 | 292 | 293 | 296 | 297 | 301
  | 305 | 306 | 315 | 316 | 317 | 318 | 319 | 320 | 321 | 322 | 323
  | 332 | 333 | 334 | 336 | 337 | 338 | 339 | 340 | 341 | 342 | 347
  | 512 | 515 | 517 | 518 | 529 | 530 | 531 | 532
  | 33421 | 33422 | 33432 | 33723 | 34377 | 34675
  | 50741 | 50778 | 50779 | 50780 | 50781 | 50879 | 50931 | 50932
  | 50981 | 50982 | 51008 | 51009 | 51022 | 51041
  | 51089 | 51090 | 51091 | 51125 -> true
  | tag when tag >= 50706 && tag <= 50739 -> true
  | tag when tag >= 50827 && tag <= 50834 -> true
  | tag when tag >= 50934 && tag <= 50942 -> true
  | tag when tag >= 50964 && tag <= 50975 -> true
  | tag when tag >= 51107 && tag <= 51112 -> true
  | _ -> false

let exif_value_tag = function
  | 33434 | 33437 | 34850 | 34852 | 34855 | 34856 | 34864 | 34865
  | 34866 | 34867 | 34868 | 34869 | 36864 | 36867 | 36868 | 36880
  | 36881 | 36882 | 37121 | 37122 | 37377 | 37378 | 37379 | 37380
  | 37381 | 37382 | 37383 | 37384 | 37385 | 37386 | 37396 | 37510
  | 37520 | 37521 | 37522 | 40960 | 40961 | 40962 | 40963 | 40964
  | 41483 | 41484 | 41486 | 41487 | 41488 | 41492 | 41493 | 41495
  | 41728 | 41729 | 41730 | 41985 | 41986 | 41987 | 41988 | 41989
  | 41990 | 41991 | 41992 | 41993 | 41994 | 41995 | 41996 | 42016
  | 42032 | 42033 | 42034 | 42035 | 42036 | 42037 -> true
  | _ -> false

let child_kind kind tag = match kind, tag with
  | Image, 330 -> Some Image
  | Image, 34665 -> Some Exif
  | Image, 34853 -> Some Gps
  | Exif, 40965 -> Some Interop
  | _ -> None

(* Camera-profile fields are self-contained values, never generic image or
   private pointer fields. The CR container has its own classic TIFF layout. *)
let camera_profile_tag = function
  | 50708 | 50721 | 50722 | 50725 | 50726 | 50778 | 50779 | 50932
  | 50936 | 50937 | 50938 | 50939 | 50940 | 50941 | 50942 | 50964
  | 50965 | 50981 | 50982 | 51107 | 51108 | 51109 | 51110 -> true
  | _ -> false

let camera_profile_type tag typ = match tag with
  | 50708 -> typ = 2
  | 50932 | 50936 | 50942 -> typ = 1 || typ = 2
  | 50778 | 50779 -> typ = 3
  | 50721 | 50722 | 50725 | 50726 | 50964 | 50965 -> typ = 10
  (* The specification calls this RATIONAL; the SDK writes SRATIONAL.
     Both are self-contained eight-byte rational values. *)
  | 51109 -> typ = 5 || typ = 10
  | 50937 | 50941 | 50981 | 51107 | 51108 | 51110 -> typ = 4
  | 50938 | 50939 | 50940 | 50982 -> typ = 11
  | _ -> false

let allowed_tag kind tag = match kind with
  | Image -> image_value_tag tag || exif_value_tag tag ||
      List.mem tag [273;279;324;325;513;514;700;330;34665;34853]
  | Exif -> exif_value_tag tag || tag = 40965
  | Gps -> tag >= 0 && tag <= 31
  | Interop -> List.mem tag [1;2;4096;4097;4098]
  | CameraProfile -> camera_profile_tag tag

(* DNG 1.7 scalar fields observed in SDK-produced JPEG XL containers.
   Only /1 admits these exact self-contained scalar shapes. They do not
   introduce pointers or authorize interpreting/decoding image payloads. *)
let extended_image_scalar tag typ = match tag with
  | 52547 -> Some (typ = 3 || typ = 4) (* ColumnInterleaveFactor *)
  | 52553 -> Some (typ = 11) (* JXLDistance *)
  | 52554 | 52555 -> Some (typ = 4) (* JXLEffort, JXLDecodeSpeed *)
  | _ -> None

type component_role = Image_bytes | Standard_value | Private_value | Structure
type component_region = { logical_id : string; schema : string; role : component_role;
                           offset : int64; length : int64 }

(* Merge-only /2 widens /1 by two opaque maker fields, the Exif MakerNote and
   the primary IFD's DNGPrivateData, and two self-contained LONG scalars seen in
   converted camera DNGs: TIFF/EP ImageNumber and DNG 1.7 CacheVersion, plus
   the DNG 1.6 ProfileGainTableMap (an UNDEFINED table). Any other field of a
   simple numeric or text type is admitted too, as are fields the owner names
   in dngmergeadmit, and XMP packets of sub-images (iPhone portrait and
   segmentation mattes carry their own); only the primary packet is editable.
   All are
   protected extents like any other value, so the complement proof keeps them
   identical and the XMP region may not overlap them. Internal pointers,
   including DNGPrivateData's original offset, are not interpreted; nothing
   outside XMP moves or changes. *)
let directory_name = function
  | Image -> "Image" | Exif -> "Exif" | Gps -> "GPS" | Interop -> "Interop"
  | CameraProfile -> "CameraProfile"

(* Simple values: ASCII, SHORT, RATIONAL, SSHORT, SRATIONAL, FLOAT and DOUBLE
   carry only numbers or text. TIFF writers hold offsets and private structures
   in LONG, BYTE, UNDEFINED or IFD fields, so those stay named one by one. *)
let simple_type typ = List.mem typ [2; 3; 5; 8; 10; 11; 12]

(* Fields admitted at run time (dngmergeadmit), as (directory, tag, type). The
   owner replaces the immutable list; workers only read it. *)
let admitted_fields : (string * int * int) list Atomic.t = Atomic.make []
let admit_fields fields = Atomic.set admitted_fields fields

let parse_admission spec =
  match String.split_on_char ':' spec with
  | [directory; tag; typ] when List.mem directory ["Image"; "Exif"; "GPS"; "Interop"] ->
      (match int_of_string_opt tag, int_of_string_opt typ with
       | Some tag, Some typ when tag >= 0 && tag < 65536 && typ >= 1 && typ <= 18
                                && typ <> 13 && typ <> 18 -> Ok (directory, tag, typ)
       | _ -> Error ("bad tag or type in " ^ spec))
  | _ -> Error ("expected Image|Exif|GPS|Interop:TAG:TYPE, got " ^ spec)

let merge_only_field kind tag typ n = match kind, tag with
  | Exif, 37500 -> Some (typ = 7 || typ = 1)
  | Exif, 42082 -> Some (typ = 7)
  | Image, 50740 -> Some (typ = 1)
  | Image, (37393 | 51114) -> Some (typ = 4 && n = 1L)
  | Image, 37398 -> Some (typ = 1 && n = 4L)
  | Image, (52525 | 52545) -> Some (typ = 7)
  | CameraProfile, _ -> None
  | _ when simple_type typ -> Some true
  | _ when List.mem (directory_name kind, tag, typ) (Atomic.get admitted_fields) -> Some true
  | _ -> None

let walk ?(record=(fun (_:component_region) -> ())) ?unknown
    {extended; opaque_maker; layout_only; require_xmp; max_xmp} reader =
 try
  let size = reader.size in
  let byte_order header = match String.sub header 0 2 with
    | "II" -> true | "MM" -> false | _ -> reject "not TIFF" in
  let number little s pos n =
    let v = ref 0L in
    for j = 0 to n-1 do
      let i = if little then n-1-j else j in
      v := Int64.logor (Int64.shift_left !v 8) (Int64.of_int (Char.code s.[pos+i]))
    done;
    (* File positions and sizes use signed int64 throughout the exact core.
       Reject unsigned 64-bit encodings outside that domain before arithmetic. *)
    if !v < 0L then reject "unsigned TIFF integer exceeds signed int64";
    !v in
  let header = reader.read 0L 8 in
  let little = byte_order header in
  let layout,header_size,primary = match number little header 2 2 with
    | 42L -> classic,8,number little header 4 4
    | 43L when extended ->
        if number little header 4 2 <> 8L || number little header 6 2 <> 0L then
          reject "invalid BigTIFF header";
        let header = reader.read 0L 16 in
        bigtiff,16,number little header 8 8
    | _ -> reject "unsupported TIFF version (including BigTIFF)" in
  let extent offset length =
    if offset < 0L || length < 0L || offset > size || length > Int64.sub size offset then
      reject "TIFF extent outside file";
    (offset, Int64.add offset length) in
  let relative base offset =
    if base < 0L || base > size || offset < 0L || offset > Int64.sub size base then
      reject "TIFF relative offset outside file";
    Int64.add base offset in
  let protected = ref [extent 0L (Int64.of_int header_size)] and extent_count = ref 1 in
  let protect offset length =
    incr extent_count;
    if !extent_count > 131072 then reject "protected extent limit";
    protected := extent offset length :: !protected in
  let record_region ?(schema="tiff-structure/0") logical_id role offset length =
    if length>0L then record {logical_id;schema;role;offset;length} in
  record_region "header" Structure 0L (Int64.of_int header_size);
  let directory_index=ref 0 and camera_header_index=ref 0 in
  let seen = Hashtbl.create 32 and queue = Queue.create () in
  let queue_room () =
    (* Camera headers and directories share the same visited/pending ceiling. *)
    if Queue.length queue + Hashtbl.length seen >= 256 then reject "IFD work limit" in
  let enqueue kind little layout base offset = if offset <> 0L then begin
    queue_room ();
    let offset = relative base offset in
    Queue.add (Directory {kind;little;layout;base;offset}) queue
  end in
  let enqueue_camera offset =
    if offset = 0L then reject "zero camera profile offset";
    queue_room ();
    ignore (extent offset 8L);
    Queue.add (CameraHeader offset) queue in
  let visit offset =
    if Hashtbl.mem seen offset then reject "repeated or cyclic IFD";
    if Hashtbl.length seen >= 256 then reject "IFD limit";
    Hashtbl.add seen offset () in
  enqueue Image little layout 0L primary;
  let entries = ref 0 and xmp = ref None and dng = ref false in
  while not (Queue.is_empty queue) do
    match Queue.take queue with
    | CameraHeader base ->
        visit base;
        protect base 8L;
        record_region ("camera-header/"^string_of_int !camera_header_index) Structure base 8L;
        incr camera_header_index;
        let header = reader.read base 8 in
        let little = byte_order header in
        if number little header 2 2 <> 0x4352L then reject "invalid camera profile header";
        let offset = number little header 4 4 in
        if offset < 8L then reject "invalid camera profile IFD offset";
        enqueue CameraProfile little classic base offset
    | Directory context ->
    let {kind;little;layout;base;offset} = context in
    let number = number little in
    let logical_directory="ifd/"^string_of_int !directory_index in
    incr directory_index;
    visit offset;
    let count64 = number (reader.read offset layout.count_width) 0 layout.count_width in
    if count64 > Int64.of_int (16384 - !entries) then reject "TIFF entry limit";
    let count = Int64.to_int count64 in
    if kind = CameraProfile && count = 0 then reject "empty camera profile IFD";
    entries := !entries + count;
    let length = layout.count_width + layout.entry_size*count + layout.number_width in
    protect offset (Int64.of_int length);
    record_region (logical_directory^"/count") Structure offset (Int64.of_int layout.count_width);
    record_region (logical_directory^"/next") Structure
      (Int64.add offset (Int64.of_int (layout.count_width+layout.entry_size*count)))
      (Int64.of_int layout.number_width);
    let directory = reader.read offset length in
    let tags = Hashtbl.create count in
    for i = 0 to count-1 do
      let at = layout.count_width+layout.entry_size*i in
      let tag = Int64.to_int (number directory at 2) in
      let typ = Int64.to_int (number directory (at+2) 2) in
      let n = number directory (at+4) layout.number_width in
      if Hashtbl.mem tags tag then reject "duplicate TIFF tag";
      let profile_pointer = extended && kind = Image && tag = 50933 in
      let scalar = if extended && kind = Image then extended_image_scalar tag typ else None in
      (match scalar with
       | Some valid when not valid || n <> 1L -> reject "invalid extended DNG scalar type or count"
       | _ -> ());
      let merge_only = opaque_maker && merge_only_field kind tag typ n <> None in
      if merge_only && merge_only_field kind tag typ n = Some false then
        reject "invalid merge-only field type or count";
      if merge_only && tag = 50740 && offset <> primary then
        reject "DNGPrivateData outside primary IFD";
      if not (layout_only || allowed_tag kind tag || profile_pointer || scalar <> None || merge_only) then
        (match unknown with
         | Some note -> note (directory_name kind) tag typ n
         | None -> reject (Printf.sprintf "unsupported TIFF tag %d (opaque or private graph)" tag));
      let child = child_kind kind tag in
      if not layout_only && (typ = 13 || (extended && typ = 18)) && child = None then
        reject "IFD type on non-pointer field";
      if kind = CameraProfile && (not layout_only || allowed_tag kind tag)
         && not (camera_profile_type tag typ) then
        reject "unsupported camera profile field type";
      let width = match typ with
        | 1 | 2 | 6 | 7 -> 1 | 3 | 8 -> 2 | 4 | 9 | 11 | 13 -> 4
        | 5 | 10 | 12 -> 8
        | 16 | 17 | 18 when layout.big -> 8
        | _ -> reject "unsupported TIFF field type" in
      if n > Int64.div Int64.max_int (Int64.of_int width) then
        reject "TIFF field size overflow";
      let bytes = Int64.mul n (Int64.of_int width) in
      let value_offset = if bytes <= Int64.of_int layout.inline_width then
          Int64.add offset (Int64.of_int (at+layout.value_at))
        else relative base (number directory (at+layout.value_at) layout.number_width) in
      ignore (extent value_offset bytes);
      Hashtbl.add tags tag (typ,n,value_offset);
      let logical_entry=logical_directory^"/tag/"^string_of_int tag in
      let value_role=if allowed_tag kind tag || profile_pointer || scalar<>None
        then Standard_value else Private_value in
      record_region (logical_entry^"/descriptor") Structure
        (Int64.add offset (Int64.of_int at)) (Int64.of_int layout.value_at);
      record_region ~schema:(Printf.sprintf "tiff-value/0:%s:%d:%Ld"
        (if little then "II" else "MM") typ n)
        (logical_entry^"/value") value_role value_offset bytes;
      let inline_start=Int64.add offset (Int64.of_int (at+layout.value_at)) in
      if bytes>Int64.of_int layout.inline_width then
        record_region (logical_entry^"/offset") Structure inline_start (Int64.of_int layout.inline_width)
      else record_region (logical_entry^"/padding") Structure (Int64.add inline_start bytes)
        (Int64.sub (Int64.of_int layout.inline_width) bytes);
      (* In /2 an auxiliary image's own packet, such as a portrait matte's, is
         an ordinary protected value; only the primary IFD's packet is edited. *)
      if tag = 700 && not (opaque_maker && offset <> primary) then begin
        if !xmp <> None then reject "multiple XMP packets";
        (* Lightroom mobile writes the primary packet as UNDEFINED; only /2 takes it. *)
        if not (typ = 1 || (opaque_maker && typ = 7)) || n <= Int64.of_int layout.inline_width ||
           n > Int64.of_int max_xmp then
          reject "unsupported XMP type or packet size";
        xmp := Some {offset = value_offset; length = Int64.to_int n}
      end else if bytes > Int64.of_int layout.inline_width then protect value_offset bytes;
      if tag = 50706 then begin
        if typ <> 1 || n <> 4L then reject "invalid DNGVersion";
        let version = reader.read value_offset 4 in
        if Char.code version.[0] <> 1 || Char.code version.[1] > 7 then
          reject "unsupported DNG version";
        if kind = Image && offset = primary then dng := true
      end;
      begin match child with
      | None -> ()
      | Some child_kind ->
        let pointer_width = match typ with
          | 4 | 13 -> 4 | 16 | 18 when layout.big -> 8
          | _ -> reject "invalid IFD pointer type" in
        if tag <> 330 && n <> 1L then reject "invalid singleton IFD pointer count";
        if n > 256L then reject "IFD pointer limit";
        let pointers = reader.read value_offset (pointer_width * Int64.to_int n) in
        for j = 0 to Int64.to_int n - 1 do
          enqueue child_kind little layout base (number pointers (pointer_width*j) pointer_width)
        done
      end;
      if profile_pointer then begin
        if offset <> primary then reject "camera profile list outside primary IFD";
        let pointer_width = match typ with
          | 4 -> 4 | 16 when layout.big -> 8
          | _ -> reject "invalid camera profile pointer type" in
        if n = 0L || n > 256L then reject "camera profile pointer limit";
        let pointers = reader.read value_offset (pointer_width * Int64.to_int n) in
        for j = 0 to Int64.to_int n - 1 do
          enqueue_camera (number pointers (pointer_width*j) pointer_width)
        done
      end
    done;
    let values tag =
      let typ,n,off = Hashtbl.find tags tag in
      if n > 65536L then reject "image extent limit";
      let width = match typ with
        | 3 -> 2 | 4 -> 4 | 16 when layout.big -> 8
        | _ -> reject "invalid image extent type" in
      let data = reader.read off (Int64.to_int n * width) in
      (n, fun i -> number data (Int64.to_int i * width) width)
    in
    List.iter (fun (offsets,lengths) ->
      let has_o = Hashtbl.mem tags offsets and has_l = Hashtbl.mem tags lengths in
      if has_o <> has_l then reject "unpaired image extents";
      if has_o then begin
        let n,get_o = values offsets and m,get_l = values lengths in
        if n <> m then reject "mismatched image extents";
        let i = ref 0L in
        while !i < n do
          let start=relative base (get_o !i) and length=get_l !i in
          protect start length;
          if kind=Image then
            record_region ~schema:"tiff-encoded-unit/0"
              (logical_directory^"/image/"^string_of_int offsets^"/"^Int64.to_string !i)
              Image_bytes start length;
          i := Int64.succ !i
        done
      end) [273,279;324,325;513,514];
    let next = number directory (layout.count_width+layout.entry_size*count) layout.number_width in
    if kind = CameraProfile && next <> 0L then reject "unsupported camera profile IFD chain";
    enqueue kind little layout base next
  done;
  if not !dng then reject "missing DNGVersion";
  let regions = match !xmp with
    | None when require_xmp -> reject "no embedded XMP"
    | None -> [] | Some region -> [region] in
  List.iter (fun (region:xmp) ->
  let stop = Int64.add region.offset (Int64.of_int region.length) in
  List.iter (fun (a,b) ->
    if a < stop && region.offset < b then reject "XMP aliases protected extent") !protected) regions;
  reader.check ();
  Ok !xmp
 with Unsupported s -> Error s

let v0 ~max_xmp = {extended=false; opaque_maker=false; layout_only=false; require_xmp=true; max_xmp}
let v1 ~max_xmp = {(v0 ~max_xmp) with extended=true}
let merge_only ~max_xmp = {(v1 ~max_xmp) with opaque_maker=true}
let layout ~max_xmp = {(v1 ~max_xmp) with layout_only=true}

(* Survey only: the same /2 walk, noting each field outside its grammar instead
   of stopping at the first. A noted field is then walked as a generic value, so
   later findings are advisory; the verdict stays with the /2 walk itself. *)
let unknown_fields ~max_xmp reader =
  let found = ref [] in
  let note directory tag typ n = found := (directory, tag, typ, n) :: !found in
  let stop = match walk ~unknown:note (merge_only ~max_xmp) reader with
    | Ok _ -> None | Error reason -> Some reason in
  List.rev !found, stop

(* Advisory physical coverage, not a transformation capability. No embedded XMP
   is required; field descriptors and padding remain structural. *)
let component_regions ~max_xmp reader =
  let regions=ref [] and count=ref 0 in
  let record region =
    incr count;
    if !count>65536 then reject "DNG raw component region limit";
    regions:=region::!regions in
  match walk ~record {(layout ~max_xmp) with require_xmp=false} reader with
  | Error reason -> Error reason
  | Ok _ -> Ok(List.rev !regions)
