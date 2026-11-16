//! Compiles the WGSL shaders in `shaders/` to SPIR-V with naga.
//!
//! Each `<name>.wgsl` becomes `$OUT_DIR/<name>.spv`, holding every entry
//! point of the file. The renderer embeds the words with `include_bytes!`.

use std::path::Path;

use naga::back::spv;
use naga::valid::{Capabilities, ValidationFlags, Validator};

const SHADERS: &[&str] = &["opaque"];

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    for name in SHADERS {
        let path = format!("shaders/{name}.wgsl");
        println!("cargo:rerun-if-changed={path}");
        let source = std::fs::read_to_string(&path).expect("read shader source");
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string_with_path(&source, &path)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::IMMEDIATES)
            .validate(&module)
            .unwrap_or_else(|e| panic!("{path}: validation failed: {e:?}"));
        let options = spv::Options {
            lang_version: (1, 3),
            // No ADJUST_COORDINATE_SPACE: the projection matrix already
            // flips y for Vulkan (lot 2.2), naga must not flip it again.
            flags: spv::WriterFlags::LABEL_VARYINGS,
            ..spv::Options::default()
        };
        let words = spv::write_vec(&module, &info, &options, None)
            .unwrap_or_else(|e| panic!("{path}: SPIR-V output failed: {e}"));
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(Path::new(&out_dir).join(format!("{name}.spv")), bytes)
            .expect("write SPIR-V");
    }
}
