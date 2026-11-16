//! Compiles every WGSL root file of `src/` to SPIR-V, one blob per entry
//! point, and generates the constants that embed them.
//!
//! A root file declares at least one entry point; other files are only
//! reached through `#include`. `<file>.wgsl` with a vertex entry point
//! becomes `<FILE>_VS` (`_FS` fragment, `_CS` compute); when a file has
//! several entry points of one stage, each is named `<FILE>_<ENTRY>`.
//! Any shader error panics, which fails the build.

#[path = "src/compile.rs"]
mod compile;
#[path = "src/preprocess.rs"]
#[allow(dead_code)]
mod preprocess;

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let src = Path::new(&manifest).join("src");
    println!("cargo:rerun-if-changed=src");

    let mut names: Vec<String> = std::fs::read_dir(&src)
        .expect("read shader directory")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".wgsl"))
        .collect();
    names.sort();

    let mut generated = String::new();
    for name in &names {
        let text = std::fs::read_to_string(src.join(name)).expect("read shader source");
        if !compile::has_entry_point(&text) {
            continue;
        }
        let pre = preprocess::preprocess_file(&src, name).unwrap_or_else(|e| panic!("{e}"));
        let label = src.join(name).display().to_string();
        let blobs = compile::compile_wgsl(&pre.source, &label).unwrap_or_else(|e| panic!("{e}"));
        let stem = name.trim_end_matches(".wgsl");
        for blob in &blobs {
            let same_stage = blobs.iter().filter(|b| b.stage == blob.stage).count();
            let suffix = if same_stage == 1 {
                blob.stage.suffix().to_owned()
            } else {
                blob.entry.to_uppercase()
            };
            let constant = format!("{}_{suffix}", stem.to_uppercase().replace('-', "_"));
            let file = Path::new(&out_dir).join(format!("{constant}.spv"));
            let bytes: Vec<u8> = blob.words.iter().flat_map(|w| w.to_le_bytes()).collect();
            std::fs::write(&file, bytes).expect("write SPIR-V");
            writeln!(
                generated,
                "/// SPIR-V of `{}` in `{name}` ({:?} stage).\npub static {constant}: &[u32] = spirv!({:?});",
                blob.entry,
                blob.stage,
                file.display().to_string(),
            )
            .expect("write to string");
        }
    }
    std::fs::write(Path::new(&out_dir).join("blobs.rs"), generated).expect("write blobs.rs");
}
