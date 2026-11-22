//! Prints the time-to-kill table of the shipped data:
//! `balance [--format text|csv] [data-dir]`. Run it through `cargo xtask balance`.
use std::path::PathBuf;

use anyhow::{bail, Result};
use faset_mode_fps::balance::{report, Format};
use faset_mode_fps::class::Catalog;

fn main() -> Result<()> {
    let mut format = Format::Text;
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--format" {
            format = match args.next().as_deref() {
                Some("text") => Format::Text,
                Some("csv") => Format::Csv,
                other => bail!("--format expects text or csv, got {other:?}"),
            };
        } else {
            dir = PathBuf::from(a);
        }
    }
    print!("{}", report(&Catalog::load_dir(&dir)?, format));
    Ok(())
}
