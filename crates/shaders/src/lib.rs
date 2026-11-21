//! WGSL shaders of the renderer, compiled to SPIR-V by `build.rs` and
//! embedded in the binary.
//!
//! Sources live in `src/*.wgsl`; `#include "name.wgsl"` shares code between
//! them (see [`preprocess`]). Each entry point is a separate blob, named
//! `<FILE>_VS`, `<FILE>_FS` or `<FILE>_CS`, e.g. [`OPAQUE_VS`].
//!
//! With the `runtime` feature, [`compile`] recompiles sources at run time
//! (shader hot reload in dev builds).

#[cfg(target_endian = "big")]
compile_error!("embedded SPIR-V is little-endian");

#[cfg(feature = "runtime")]
pub mod compile;
pub mod preprocess;

/// Directory holding the WGSL sources, as seen when this crate was built.
pub const SOURCE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

/// Embeds a SPIR-V file as 4-byte aligned words.
macro_rules! spirv {
    ($path:literal) => {{
        #[repr(C, align(4))]
        struct Aligned<B: ?Sized>(B);
        static BYTES: &Aligned<[u8]> = &Aligned(*include_bytes!($path));
        // SAFETY: `BYTES` is 4-byte aligned and static; SPIR-V is a whole
        // number of words; every bit pattern is a valid `u32`.
        unsafe { ::core::slice::from_raw_parts(BYTES.0.as_ptr().cast::<u32>(), BYTES.0.len() / 4) }
    }};
}

include!(concat!(env!("OUT_DIR"), "/blobs.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    const SPIRV_MAGIC: u32 = 0x0723_0203;

    #[test]
    fn opaque_blobs_are_spirv() {
        for blob in [OPAQUE_VS, OPAQUE_FS] {
            assert!(blob.len() > 5);
            assert_eq!(blob[0], SPIRV_MAGIC);
        }
        assert_ne!(OPAQUE_VS, OPAQUE_FS);
    }

    #[test]
    fn shadow_vertex_blob_is_spirv() {
        assert_eq!(SHADOW_VS[0], SPIRV_MAGIC);
        assert!(SHADOW_VS.len() > 5);
    }

    #[test]
    fn sources_are_present() {
        let dir = std::path::Path::new(SOURCE_DIR);
        let pre = preprocess::preprocess_file(dir, "opaque.wgsl").unwrap();
        assert!(
            pre.files.len() > 1,
            "opaque.wgsl includes the material code"
        );
        assert!(!pre.source.contains("#include"));
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_compile_matches_build() {
        let dir = std::path::Path::new(SOURCE_DIR);
        let pre = preprocess::preprocess_file(dir, "opaque.wgsl").unwrap();
        let blobs = compile::compile_wgsl(&pre.source, "opaque.wgsl").unwrap();
        let vs = blobs.iter().find(|b| b.stage == compile::Stage::Vertex);
        assert_eq!(vs.unwrap().words, OPAQUE_VS);
        assert!(compile::compile_wgsl("fn broken( {", "broken.wgsl").is_err());
    }
}
