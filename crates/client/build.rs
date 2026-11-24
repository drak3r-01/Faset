//! Embeds the prop recipes of `assets/props` (lot 6.11): generates
//! `props.rs`, a list of `(relative path, RON text)` pairs, so that the client
//! draws every prop of a map without reading the asset folder at run time.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "ron")
            && path.file_name().is_some_and(|n| n != "palette.ron")
        {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let props = Path::new(&manifest).join("../../assets/props");
    println!("cargo:rerun-if-changed={}", props.display());
    let mut files = Vec::new();
    collect(&props, &mut files);
    files.sort();
    let mut generated =
        String::from("/// Embedded prop recipes: (path under `assets/props`, RON).\n");
    generated.push_str("pub(crate) const PROP_RECIPES: &[(&str, &str)] = &[\n");
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let rel = file
            .strip_prefix(&props)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let abs = file
            .canonicalize()
            .unwrap_or_else(|_| file.clone())
            .to_string_lossy()
            .into_owned();
        writeln!(generated, "    ({rel:?}, include_str!({abs:?})),").expect("write to string");
    }
    generated.push_str("];\n");
    std::fs::write(Path::new(&out_dir).join("props.rs"), generated).expect("write props.rs");
}
