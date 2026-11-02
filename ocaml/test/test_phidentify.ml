(* Synthetic files only: byte identity, a non-DNG skipping raw-block identity,
   a missing file, and the parallel pool returning results in input order. *)
let write path contents = Out_channel.with_open_bin path (fun oc -> output_string oc contents)

let () =
  let dir = Filename.concat (Filename.get_temp_dir_name ()) (Printf.sprintf "phidentify-ml-%d" (Unix.getpid ())) in
  Sys.mkdir dir 0o700;
  let a = Filename.concat dir "a.jpg" and b = Filename.concat dir "b.jpg" in
  write a "\xff\xd8\xff\xe0 synthetic one";
  write b "\xff\xd8\xff\xe0 synthetic two";
  let request = { Phidentify.dng_raw_blocks = true; lightroom = true; metadata = false } in
  (match Phidentify.identify ~request a with
   | Ok i ->
     assert (String.length i.sha256 = 64);
     assert (i.dng_raw_blocks = Phidentify.Not_attempted);
     assert (i.lightroom = Phidentify.Not_attempted);
     assert (i.metadata = Phidentify.Not_attempted)
   | Error e -> failwith e);
  (match Phidentify.identify (Filename.concat dir "missing") with
   | Error _ -> ()
   | Ok _ -> failwith "missing file identified");
  let paths = List.concat (List.init 20 (fun _ -> [ a; b ])) in
  let results = Phidentify.identify_all ~request ~workers:4 paths in
  let shas = List.map (function Ok (i : Phidentify.identity) -> i.sha256 | Error e -> failwith e) results in
  let sha_a = List.hd shas and sha_b = List.nth shas 1 in
  assert (sha_a <> sha_b);
  List.iteri (fun k s -> assert (s = if k mod 2 = 0 then sha_a else sha_b)) shas;
  Sys.remove a; Sys.remove b; Sys.rmdir dir;
  Printf.printf "ok %s %s\n" (Phidentify.version ()) Phidentify.backend
