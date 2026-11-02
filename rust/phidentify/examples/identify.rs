//! Print every identity of each path argument as one JSON line.
use phidentify::{Options, identify};
use std::path::Path;

fn main() {
    let opts = Options { dng_blocks: true, lightroom: true, metadata: cfg!(feature = "metadata"), preserved_regions: None, dng_semantic: true };
    for arg in std::env::args().skip(1) {
        match identify(Path::new(&arg), opts.clone()) {
            Ok(id) => println!("{}", serde_json::json!({"path": arg, "identity": id})),
            Err(e) => println!("{}", serde_json::json!({"path": arg, "error": format!("{e:#}")})),
        }
    }
}
