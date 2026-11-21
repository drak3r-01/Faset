//! `asset-tool validate <path> [--out <dir>] [--no-thumbnails]`
//!
//! Checks syntax, budget and dimensions of each recipe, renders a thumbnail
//! of each without a window and writes `contact_sheet.png` plus one PNG per
//! prop into the output directory (default `asset-tool-out`). Exits with a
//! non-zero code when any recipe fails.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use asset_tool::{contact_sheet, load_palette, thumbnail, validate_path, Thumbnailer, THUMB_SIZE};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match run(std::env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "usage: asset-tool validate <path> [--out <dir>] [--no-thumbnails]";

fn run(args: Vec<String>) -> Result<bool> {
    let mut it = args.into_iter();
    if it.next().as_deref() != Some("validate") {
        bail!(USAGE);
    }
    let mut path = None;
    let mut out = PathBuf::from("asset-tool-out");
    let mut thumbnails = true;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().context(USAGE)?.into(),
            "--no-thumbnails" => thumbnails = false,
            _ if path.is_none() => path = Some(PathBuf::from(a)),
            _ => bail!(USAGE),
        }
    }
    let path = path.context(USAGE)?;
    let palette = load_palette(&path)?;
    let entries = validate_path(&path, &palette)?;
    if entries.is_empty() {
        bail!("no recipes found in {}", path.display());
    }

    let mut failed = 0;
    for e in &entries {
        let v = &e.validation;
        let status = if v.is_ok() { "ok  " } else { "FAIL" };
        println!(
            "{status} {:<24} {:>4} tris {:>4} verts  {:.2} x {:.2} x {:.2} m",
            v.id, v.triangles, v.vertices, v.size[0], v.size[1], v.size[2]
        );
        for msg in &v.errors {
            println!("       {}: {msg}", e.path.display());
        }
        failed += usize::from(!v.is_ok());
    }

    if thumbnails {
        std::fs::create_dir_all(&out)?;
        let mut renderer = Thumbnailer::new(THUMB_SIZE)?;
        let mut thumbs = Vec::new();
        for e in entries.iter().filter(|e| e.validation.is_ok()) {
            let Some(recipe) = &e.recipe else { continue };
            let img = thumbnail(&mut renderer, recipe, &palette)
                .with_context(|| format!("thumbnail of {}", recipe.id))?;
            img.save(out.join(format!("{}.png", recipe.id)))?;
            thumbs.push(img);
        }
        let sheet = out.join("contact_sheet.png");
        contact_sheet(&thumbs, 5).save(&sheet)?;
        println!("contact sheet: {}", sheet.display());
        if renderer.validation_message_count() > 0 {
            eprintln!(
                "{} Vulkan validation messages",
                renderer.validation_message_count()
            );
            failed += 1;
        }
    }
    println!("{} recipes, {failed} failed", entries.len());
    Ok(failed == 0)
}
