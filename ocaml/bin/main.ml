(* phidentify [-j N] PATH...: one tab-separated line per path:
   path, sha256, dng_raw_blocks, lightroom, metadata, dng_semantic profile,
   preserved, image_data, block_marks (each "-" when not attempted, "!reason"
   when failed). *)
let field = function
  | Phidentify.Not_attempted -> "-"
  | Ok v -> v
  | Failed reason -> "!" ^ String.map (function '\t' | '\n' -> ' ' | c -> c) reason

let () =
  let args = List.tl (Array.to_list Sys.argv) in
  let workers, paths = match args with
    | "-j" :: n :: rest -> int_of_string n, rest
    | rest -> Phidentify.recommended_workers (), rest in
  let request = { Phidentify.all with metadata = false } in
  List.iter2 (fun path r -> match r with
    | Ok (i : Phidentify.identity) ->
      Printf.printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" path i.sha256 (field i.dng_raw_blocks) (field i.lightroom)
        (field i.metadata) (field i.dng_semantic) (field i.preserved) (field i.image_data) (field i.block_marks)
    | Error e -> Printf.printf "%s\t!%s\n" path e)
    paths (Phidentify.identify_all ~request ~workers paths)
