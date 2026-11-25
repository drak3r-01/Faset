//! Developer task runner: `cargo xtask <command>`.
//!
//! All build, test and quality logic lives here so that the host scripts
//! (`dev.ps1`, `dev.sh`) stay thin wrappers around Docker.
#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

const USAGE: &str =
    "usage: cargo xtask <check|test|render-test|build --target <t>|package [--target <t>]|check-tag <tag>|maps validate|balance [--format text|csv]|deny|ci>";

/// Targets built in the dev image. macOS targets are built by CI runners (lot 0.4).
const TARGETS: [&str; 4] = [
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
];

const DIST: &str = "dist";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&args)
}

fn run(args: &[&str]) -> Result<()> {
    match args {
        ["check"] => check(),
        ["test"] => test(),
        ["render-test"] => render_test(),
        ["build", "--target", target] => build(target),
        ["package"] => package(None),
        ["package", "--target", target] => package(Some(target)),
        ["check-tag", tag] => check_tag(tag),
        ["maps", "validate"] => maps_validate(),
        ["balance"] => balance(&[]),
        ["balance", "--format", f] => balance(&["--format", f]),
        ["deny"] => deny(),
        ["ci"] => ci(),
        _ => bail!("unknown command {args:?}\n{USAGE}"),
    }
}

/// Runs `cargo <args>` and fails if it exits with a non-zero status.
fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    println!("+ cargo {}", args.join(" "));
    let status = Command::new(&cargo)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{cargo}`"))?;
    if !status.success() {
        bail!("`cargo {}` failed with {status}", args.join(" "));
    }
    Ok(())
}

/// `fmt --check`, then `clippy -D warnings`.
fn check() -> Result<()> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ])
}

/// Unit and integration tests through nextest.
fn test() -> Result<()> {
    cargo(&[
        "nextest",
        "run",
        "--workspace",
        "--all-features",
        "--no-tests=pass",
        "-E",
        "not binary(lavapipe)",
    ])
}

/// Headless lavapipe render tests (software Vulkan, validation layers on).
fn render_test() -> Result<()> {
    cargo(&[
        "nextest",
        "run",
        "--workspace",
        "--all-features",
        "--no-tests=pass",
        "-E",
        "binary(lavapipe)",
    ])
}

/// Runs an external program and fails on a non-zero exit.
fn run_cmd(program: &str, args: &[&str], dir: Option<&Path>) -> Result<()> {
    println!("+ {program} {}", args.join(" "));
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    if !status.success() {
        bail!("`{program}` failed with {status}");
    }
    Ok(())
}

fn is_windows(target: &str) -> bool {
    target.contains("windows")
}

/// Executables shipped per target: (package, binary name).
const BINARIES: [(&str, &str); 2] = [("faset-client", "faset"), ("dedicated", "dedicated")];

/// Data folders and files copied next to the executables of every archive.
const PAYLOAD: [&str; 2] = ["assets/maps", "crates/dedicated/server.toml"];

fn exe_name(target: &str, bin: &str) -> String {
    if is_windows(target) {
        format!("{bin}.exe")
    } else {
        bin.to_owned()
    }
}

/// Builds the client and the dedicated server for one target (a single cargo
/// invocation, incremental) and stages them with the maps into `dist/<target>/`.
fn build(target: &str) -> Result<()> {
    if !TARGETS.contains(&target) {
        bail!("unsupported target `{target}`; expected one of {TARGETS:?}");
    }
    let mut args: Vec<&str> = if is_windows(target) {
        vec!["xwin", "build"]
    } else {
        vec!["build"]
    };
    args.extend(["--release", "--target", target]);
    for (package, _) in BINARIES {
        args.extend(["-p", package]);
    }
    cargo(&args)?;

    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_owned());
    let out_dir = Path::new(DIST).join(target);
    fs::create_dir_all(&out_dir)?;
    for (_, bin) in BINARIES {
        let exe = exe_name(target, bin);
        let built = Path::new(&target_dir)
            .join(target)
            .join("release")
            .join(&exe);
        fs::copy(&built, out_dir.join(&exe))
            .with_context(|| format!("failed to copy {}", built.display()))?;
        println!("built {}", out_dir.join(&exe).display());
    }
    for item in PAYLOAD {
        copy_payload(Path::new(item), &out_dir)?;
    }
    Ok(())
}

/// Copies a file into `dir`, or a folder (flat, one level) as `dir/<folder name>/`.
fn copy_payload(src: &Path, dir: &Path) -> Result<()> {
    let name = src.file_name().context("payload without a name")?;
    if src.is_dir() {
        let dest = dir.join(name);
        fs::create_dir_all(&dest)?;
        for entry in fs::read_dir(src)? {
            let path = entry?.path();
            if path.is_file() {
                fs::copy(&path, dest.join(path.file_name().context("file name")?))
                    .with_context(|| format!("failed to copy {}", path.display()))?;
            }
        }
    } else {
        fs::copy(src, dir.join(name))
            .with_context(|| format!("failed to copy {}", src.display()))?;
    }
    Ok(())
}

fn archive_name(target: &str) -> String {
    if is_windows(target) {
        format!("faset-{target}.zip")
    } else {
        format!("faset-{target}.tar.gz")
    }
}

/// Archives the built targets (all, or one) and writes `dist/SHA256SUMS`
/// covering every archive present. Nothing is rebuilt here.
fn package(only: Option<&str>) -> Result<()> {
    if let Some(t) = only {
        if !TARGETS.contains(&t) {
            bail!("unsupported target `{t}`; expected one of {TARGETS:?}");
        }
    }
    let dist = Path::new(DIST);
    let mut count = 0;
    for target in TARGETS.iter().filter(|t| only.is_none_or(|o| o == **t)) {
        let target = *target;
        if !dist
            .join(target)
            .join(exe_name(target, BINARIES[0].1))
            .is_file()
        {
            println!("skipping {target}: not built");
            continue;
        }
        let name = archive_name(target);
        let _ = fs::remove_file(dist.join(&name));
        if is_windows(target) {
            run_cmd("zip", &["-r", "-q", &name, target], Some(dist))?;
        } else {
            run_cmd("tar", &["-czf", &name, target], Some(dist))?;
        }
        count += 1;
    }
    if count == 0 {
        bail!("nothing to package: run `build --target <t>` first");
    }
    let mut archives: Vec<String> = TARGETS
        .iter()
        .map(|t| archive_name(t))
        .filter(|n| dist.join(n).is_file())
        .collect();
    archives.sort();
    let mut sums = String::new();
    for name in &archives {
        let out = Command::new("sha256sum")
            .arg(name)
            .current_dir(dist)
            .output()
            .context("failed to spawn `sha256sum`")?;
        if !out.status.success() {
            bail!("`sha256sum {name}` failed");
        }
        sums.push_str(&String::from_utf8_lossy(&out.stdout));
    }
    fs::write(dist.join("SHA256SUMS"), sums)?;
    println!(
        "packaged {count} archive(s), checksums for {}",
        archives.len()
    );
    Ok(())
}

/// Prints the weapon x class time-to-kill table (lot 4.6).
fn balance(extra: &[&str]) -> Result<()> {
    if let [_, f] = extra {
        if !["text", "csv"].contains(f) {
            bail!("--format expects text or csv, got `{f}`");
        }
    }
    let mut args = vec![
        "run",
        "-q",
        "-p",
        "faset-mode-fps",
        "--bin",
        "balance",
        "--",
    ];
    args.extend(extra);
    cargo(&args)
}

/// Validates every native map of `assets/maps` with `map-tool`.
fn maps_validate() -> Result<()> {
    let dir = Path::new("assets/maps");
    let mut maps: Vec<_> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "ron"))
            .collect(),
        Err(_) => Vec::new(),
    };
    maps.sort();
    println!("maps validate: {} maps found", maps.len());
    let mut failed = 0;
    for map in &maps {
        let path = map.to_string_lossy();
        if cargo(&["run", "-q", "-p", "map-tool", "--", "validate", &path]).is_err() {
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} map(s) failed validation");
    }
    Ok(())
}

/// Reads `[workspace.package] version` from a root manifest's text.
fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
        } else if in_section {
            if let Some(rest) = line.strip_prefix("version") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_owned());
            }
        }
    }
    None
}

/// Fails unless the tag `vX.Y.Z[-pre]` equals the workspace version.
fn check_tag(tag: &str) -> Result<()> {
    let manifest = fs::read_to_string("Cargo.toml").context("failed to read Cargo.toml")?;
    verify_tag(tag, &manifest)
}

fn verify_tag(tag: &str, manifest: &str) -> Result<()> {
    let version = workspace_version(manifest).context("no [workspace.package] version")?;
    let Some(tag_version) = tag.strip_prefix('v') else {
        bail!("tag `{tag}` must start with `v`");
    };
    if tag_version != version {
        bail!("tag `{tag}` does not match workspace version `{version}`");
    }
    println!("tag {tag} matches workspace version {version}");
    Ok(())
}

/// Licence and advisory checks.
fn deny() -> Result<()> {
    cargo(&["deny", "check"])
}

/// Full local gate, in order; stops at the first failure.
fn ci() -> Result<()> {
    check()?;
    test()?;
    render_test()?;
    maps_validate()?;
    deny()
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn unknown_command_is_rejected() {
        assert!(run(&["nope"]).is_err());
        assert!(run(&[]).is_err());
        assert!(run(&["maps"]).is_err());
    }

    #[test]
    fn stubs_succeed() {
        assert!(run(&["maps", "validate"]).is_ok());
    }

    #[test]
    fn workspace_version_is_parsed() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nedition = \"2021\"\nversion = \"1.2.3-rc.1\"\n";
        assert_eq!(
            super::workspace_version(manifest).as_deref(),
            Some("1.2.3-rc.1")
        );
        assert_eq!(
            super::workspace_version("[package]\nversion = \"9\"\n"),
            None
        );
    }

    #[test]
    fn tag_must_match_workspace_version() {
        let version = super::workspace_version(include_str!("../../Cargo.toml")).unwrap();
        let manifest = include_str!("../../Cargo.toml");
        assert!(super::verify_tag(&format!("v{version}"), manifest).is_ok());
        assert!(super::verify_tag("v9.9.9", manifest).is_err());
        assert!(super::verify_tag(&version, manifest).is_err());
    }

    #[test]
    fn package_rejects_unknown_target() {
        assert!(run(&["package", "--target", "aarch64-apple-darwin"]).is_err());
    }

    #[test]
    fn archive_names_follow_the_target() {
        assert_eq!(
            super::archive_name("x86_64-pc-windows-msvc"),
            "faset-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            super::archive_name("aarch64-unknown-linux-gnu"),
            "faset-aarch64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            super::exe_name("x86_64-pc-windows-msvc", "faset"),
            "faset.exe"
        );
        assert_eq!(
            super::exe_name("x86_64-unknown-linux-gnu", "faset"),
            "faset"
        );
    }

    #[test]
    fn build_rejects_unknown_target() {
        assert!(run(&["build", "--target", "riscv64-unknown-none"]).is_err());
        assert!(run(&["build", "--target", "aarch64-apple-darwin"]).is_err());
    }
}
