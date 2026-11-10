//! skeleton PATH...: each file's block marks and skeleton record, one JSON line per file.
//! skeleton --rebuild OLD EDITED OUT: write OLD's content from its skeleton and EDITED.
use phidentify::skeleton;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--rebuild") {
        let old = skeleton::build(Path::new(&args[1])).expect("skeleton");
        let mut out = std::fs::File::create(&args[3]).expect("output");
        let sha = skeleton::rebuild(&old, Path::new(&args[2]), &mut out).expect("rebuild");
        let want = phidentify::file::sha256_file(Path::new(&args[1])).expect("sha256");
        println!("{}", if sha == want { "rebuilt exactly" } else { "REBUILT WRONG" });
        return;
    }
    for arg in args {
        let path = Path::new(&arg);
        match skeleton::build(path) {
            Ok(s) => {
                let sha = phidentify::file::sha256_file(path).unwrap_or_default();
                let marks = match skeleton::block_marks(&s) {
                    Ok(m) => format!("{m:?}"),
                    Err(e) => format!("null, \"block_marks_error\": {e:?}"),
                };
                println!("{{\"path\": {arg:?}, \"block_marks\": {marks}, \"record\": {}}}", s.record(&sha));
            }
            Err(e) => println!("{{\"path\": {arg:?}, \"error\": {e:?}}}"),
        }
    }
}
