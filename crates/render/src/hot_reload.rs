//! Shader hot reload (feature `hot-reload`, dev builds only).
//!
//! [`ShaderWatcher`] polls the modification time and size of every `.wgsl`
//! file of a directory. When one changes it recompiles the pipeline roots
//! with naga and swaps the pipeline in; on error the previous pipeline stays
//! and the error is logged with `tracing::error!`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use faset_shaders::compile::{compile_wgsl, Stage};
use faset_shaders::preprocess::preprocess_file;

use crate::Renderer;

/// Root file of the opaque pipeline.
const OPAQUE_ROOT: &str = "opaque.wgsl";

/// Modification time and size of each watched file.
type Stamps = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

/// Watches a WGSL source directory and reloads changed shaders.
#[derive(Debug)]
pub struct ShaderWatcher {
    dir: PathBuf,
    stamps: Stamps,
}

impl ShaderWatcher {
    /// Watches `dir`. The current files are the baseline: only later edits
    /// trigger a reload.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let stamps = scan(&dir);
        Self { dir, stamps }
    }

    /// Watches the `faset-shaders` source directory of this checkout.
    pub fn for_sources() -> Self {
        Self::new(faset_shaders::SOURCE_DIR)
    }

    /// The watched directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Checks for edits; on a change, recompiles and swaps the pipeline.
    /// Call once per frame. Returns true when a new pipeline went live.
    pub fn poll(&mut self, renderer: &mut Renderer) -> bool {
        let stamps = scan(&self.dir);
        if stamps == self.stamps {
            return false;
        }
        // Remember the new state even on failure: the next edit retries.
        self.stamps = stamps;
        match self.reload(renderer) {
            Ok(()) => {
                tracing::info!(dir = %self.dir.display(), "shaders reloaded");
                true
            }
            Err(e) => {
                tracing::error!("shader reload failed, keeping the previous pipeline: {e}");
                false
            }
        }
    }

    fn reload(&self, renderer: &mut Renderer) -> Result<(), String> {
        let pre = preprocess_file(&self.dir, OPAQUE_ROOT).map_err(|e| e.to_string())?;
        let label = self.dir.join(OPAQUE_ROOT).display().to_string();
        let blobs = compile_wgsl(&pre.source, &label)?;
        let find = |stage| {
            blobs
                .iter()
                .find(|b| b.stage == stage)
                .map(|b| b.words.as_slice())
                .ok_or_else(|| format!("{OPAQUE_ROOT}: no {stage:?} entry point"))
        };
        let (vs, fs) = (find(Stage::Vertex)?, find(Stage::Fragment)?);
        renderer
            .replace_opaque_shaders(vs, fs)
            .map_err(|e| format!("pipeline creation failed: {e}"))
    }
}

fn scan(dir: &Path) -> Stamps {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Stamps::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "wgsl"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), (meta.modified().ok(), meta.len())))
        })
        .collect()
}
