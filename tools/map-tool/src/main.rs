//! `map-tool validate <map.ron> [--report out.json] [--image out.png]`
//! `map-tool view <map.ron> [--top out.png] [--perspective out.png]`
//!
//! `validate` prints every violation and exits with 1 if there is one; the
//! JSON report and a top-view image are optional outputs. `view` only renders.
//! Exit code 2 means the tool itself failed.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use map_tool::view::{Viewer, VIEW_SIZE};

const USAGE: &str = "usage: map-tool validate <map.ron> [--report out.json] [--image out.png]\n       map-tool view <map.ron> [--top out.png] [--perspective out.png]";

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

struct Args {
    map: PathBuf,
    /// `--report` for validate, `--top` for view.
    first: Option<PathBuf>,
    /// `--image` for validate, `--perspective` for view.
    second: Option<PathBuf>,
}

fn parse(args: impl Iterator<Item = String>, flags: [&str; 2]) -> Result<Args> {
    let mut it = args;
    let (mut map, mut first, mut second) = (None, None, None);
    while let Some(a) = it.next() {
        if a == flags[0] {
            first = Some(PathBuf::from(it.next().context(USAGE)?));
        } else if a == flags[1] {
            second = Some(PathBuf::from(it.next().context(USAGE)?));
        } else if map.is_none() && !a.starts_with("--") {
            map = Some(PathBuf::from(a));
        } else {
            bail!(USAGE);
        }
    }
    Ok(Args {
        map: map.context(USAGE)?,
        first,
        second,
    })
}

fn run(args: Vec<String>) -> Result<bool> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("validate") => validate(&parse(it, ["--report", "--image"])?),
        Some("view") => view(&parse(it, ["--top", "--perspective"])?),
        _ => bail!(USAGE),
    }
}

fn validate(args: &Args) -> Result<bool> {
    let bytes =
        std::fs::read(&args.map).with_context(|| format!("cannot read {}", args.map.display()))?;
    let report = map_tool::validate_bytes(&bytes);
    let name = args.map.display();
    for e in &report.errors {
        println!("{name}: error: {e}");
    }
    for w in &report.warnings {
        println!("{name}: warning: {w}");
    }
    println!(
        "{name}: {} error(s), {} warning(s), {} triangles, {:.0} m2 playable",
        report.errors.len(),
        report.warnings.len(),
        report.stats.triangles,
        report.stats.playable_surface_m2
    );
    if let Some(path) = &args.first {
        std::fs::write(path, report.to_json())
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    if let Some(path) = &args.second {
        if report.map_id.is_none() {
            bail!("no image: the map could not be parsed");
        }
        let map = faset_client::load_map(&bytes)?;
        let mut viewer = Viewer::new(&map, VIEW_SIZE)?;
        viewer.top(&map)?.save(path)?;
    }
    Ok(report.is_ok())
}

fn view(args: &Args) -> Result<bool> {
    let bytes =
        std::fs::read(&args.map).with_context(|| format!("cannot read {}", args.map.display()))?;
    let map = faset_client::load_map(&bytes)?;
    let mut viewer = Viewer::new(&map, VIEW_SIZE)?;
    if let Some(path) = &args.first {
        viewer.top(&map)?.save(path)?;
    }
    if let Some(path) = &args.second {
        viewer.perspective()?.save(path)?;
    }
    Ok(true)
}
