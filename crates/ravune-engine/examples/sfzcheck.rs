//! Loads multi-sample instruments the way Ravune does and tells what it found in each:
//! `cargo run -p ravune-engine --example sfzcheck -- a.sfz b.dspreset …`

use std::path::Path;
use std::sync::Arc;

fn main() {
    for file in std::env::args().skip(1) {
        let mut missing = Vec::new();
        let mut load = |p: &str| match ravune_engine::Sample::decode_file(Path::new(p)) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                missing.push(e);
                None
            }
        };
        match ravune_engine::MultiMap::load(Path::new(&file), &mut load) {
            Ok(map) => {
                let (lo, hi) = map.range();
                println!("ok  {} regions, keys {lo}–{hi}, {} samples unreadable · {file}", map.regions.len(), missing.len());
            }
            Err(e) => println!("ERR {e} · {file}"),
        }
        for e in missing.iter().take(3) {
            println!("      {e}");
        }
    }
}
