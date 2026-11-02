//! Peak-memory probe: identify every path with one identity at a time.
//! memprobe MODE PATH...   MODE: sha | blocks | lightroom | metadata | all
use phidentify::{Options, identify};
use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode");
    let opts = Options {
        dng_blocks: mode == "blocks" || mode == "all",
        lightroom: mode == "lightroom" || mode == "all",
        metadata: mode == "metadata" || mode == "all",
        preserved_regions: None,
        dng_semantic: false,
        image_data: mode == "image" || mode == "all",
    };
    for arg in args {
        let _ = identify(Path::new(&arg), opts.clone());
    }
}
