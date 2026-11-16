//! WGSL -> SPIR-V compilation with naga, one module per entry point.
//!
//! Shared by `build.rs` (through `#[path]`) and, with the `runtime`
//! feature, by shader hot reload.

use naga::back::spv;
use naga::valid::{Capabilities, ValidationFlags, Validator};

/// Pipeline stage of an entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

impl Stage {
    /// Suffix of the embedded blob names (`VS`, `FS`, `CS`).
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Vertex => "VS",
            Self::Fragment => "FS",
            Self::Compute => "CS",
        }
    }
}

/// SPIR-V of one entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryBlob {
    pub entry: String,
    pub stage: Stage,
    pub words: Vec<u32>,
}

/// Parses, validates and compiles every entry point of a preprocessed WGSL
/// source. `label` names the source in error messages.
pub fn compile_wgsl(source: &str, label: &str) -> Result<Vec<EntryBlob>, String> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| e.emit_to_string_with_path(source, label))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::IMMEDIATES)
        .validate(&module)
        .map_err(|e| format!("{label}: validation failed: {}", e.emit_to_string(source)))?;
    let options = spv::Options {
        lang_version: (1, 3),
        // No ADJUST_COORDINATE_SPACE: the projection matrix already flips y
        // for Vulkan, naga must not flip it again.
        flags: spv::WriterFlags::LABEL_VARYINGS,
        ..spv::Options::default()
    };
    let mut blobs = Vec::new();
    for ep in &module.entry_points {
        let stage = match ep.stage {
            naga::ShaderStage::Vertex => Stage::Vertex,
            naga::ShaderStage::Fragment => Stage::Fragment,
            naga::ShaderStage::Compute => Stage::Compute,
            #[allow(unreachable_patterns)]
            other => return Err(format!("{label}: unsupported stage {other:?}")),
        };
        let pipeline = spv::PipelineOptions {
            shader_stage: ep.stage,
            entry_point: ep.name.clone(),
        };
        let words = spv::write_vec(&module, &info, &options, Some(&pipeline))
            .map_err(|e| format!("{label}: SPIR-V output of `{}` failed: {e}", ep.name))?;
        blobs.push(EntryBlob {
            entry: ep.name.clone(),
            stage,
            words,
        });
    }
    Ok(blobs)
}

/// True when a WGSL file declares an entry point (a root file); other
/// files are libraries reached through `#include`.
pub fn has_entry_point(source: &str) -> bool {
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        ["@vertex", "@fragment", "@compute"]
            .iter()
            .any(|attr| code.contains(attr))
    })
}
