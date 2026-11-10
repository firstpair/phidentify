(* Synthetic files only: byte identity, a non-DNG skipping raw-block identity,
   a missing file, and the parallel pool returning results in input order. *)
let write path contents = Out_channel.with_open_bin path (fun oc -> output_string oc contents)

let () =
  let dir = Filename.concat (Filename.get_temp_dir_name ()) (Printf.sprintf "phidentify-ml-%d" (Unix.getpid ())) in
  Sys.mkdir dir 0o700;
  let a = Filename.concat dir "a.jpg" and b = Filename.concat dir "b.jpg" in
  write a "\xff\xd8\xff\xe0 synthetic one";
  write b "\xff\xd8\xff\xe0 synthetic two";
  let request = { Phidentify.all with metadata = false } in
  (match Phidentify.identify ~request a with
   | Ok i ->
     assert (String.length i.sha256 = 64);
     assert (i.dng_raw_blocks = Phidentify.Not_attempted);
     assert (i.lightroom = Phidentify.Not_attempted);
     assert (i.metadata = Phidentify.Not_attempted);
     assert (match i.dng_semantic with Phidentify.Failed _ -> true | _ -> false);
     assert (i.preserved = Phidentify.Not_attempted);
     assert (match i.image_data with Phidentify.Failed _ -> true | _ -> false);
     (* A JPEG start: everything after SOI is one block, the scan. *)
     assert (match i.block_marks with Phidentify.Ok _ -> true | _ -> false);
     assert (i.skeleton = None)
   | Error e -> failwith e);
  (* A JPEG with a 700-byte APP2 and a scan: two blocks. Its skeleton comes with
     the read when asked for, and the same one from the structures alone. *)
  let c = Filename.concat dir "c.jpg" in
  let seg marker body =
    let n = String.length body + 2 in
    Printf.sprintf "\xff%c%c%c%s" marker (Char.chr (n lsr 8)) (Char.chr (n land 255)) body in
  write c ("\xff\xd8" ^ seg '\xe1' "Exif\000\000caption" ^ seg '\xe2' (String.make 700 'p')
           ^ "\xff\xda\000\008\001\001\000\000\063\000" ^ String.make 3000 's' ^ "\xff\xd9");
  (match Phidentify.identify ~request:{ request with skeleton = true } c with
   | Ok i ->
     let marks = match i.block_marks with Phidentify.Ok m -> m | _ -> failwith "no block marks" in
     assert (String.length marks = 64);
     let stored = match i.skeleton with Some (Ok s) -> s | _ -> failwith "no skeleton" in
     assert (String.starts_with ~prefix:(Printf.sprintf "{\"sha256\": %S, \"size\": " i.sha256) stored.record);
     assert (String.sub stored.packed 0 4 = "\x28\xb5\x2f\xfd");
     assert (Phidentify.skeleton ~sha256:i.sha256 c = (Phidentify.Ok marks, Some (Ok stored)))
   | Error e -> failwith e);
  (match Phidentify.skeleton ~sha256:"" (Filename.concat dir "missing") with
   | Phidentify.Failed _, Some (Error _) -> ()
   | _ -> failwith "skeleton of a missing file");
  (match Phidentify.identify (Filename.concat dir "missing") with
   | Error _ -> ()
   | Ok _ -> failwith "missing file identified");
  let paths = List.concat (List.init 20 (fun _ -> [ a; b ])) in
  let results = Phidentify.identify_all ~request ~workers:4 paths in
  let shas = List.map (function Ok (i : Phidentify.identity) -> i.sha256 | Error e -> failwith e) results in
  let sha_a = List.hd shas and sha_b = List.nth shas 1 in
  assert (sha_a <> sha_b);
  List.iteri (fun k s -> assert (s = if k mod 2 = 0 then sha_a else sha_b)) shas;
  Sys.remove a; Sys.remove b; Sys.remove c; Sys.rmdir dir;
  Printf.printf "ok %s %s\n" (Phidentify.version ()) Phidentify.backend
