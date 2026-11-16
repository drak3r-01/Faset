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
    "usage: cargo xtask <check|test|render-test|build --target <t>|package|check-tag <tag>|maps validate|deny|ci>";

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
        ["package"] => package(),
        ["check-tag", tag] => check_tag(tag),
        ["maps", "validate"] => maps_validate(),
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

fn exe_name(target: &str) -> &'static str {
    if is_windows(target) {
        "probe.exe"
    } else {
        "probe"
    }
}

/// Builds the placeholder binary for one target into `dist/<target>/`.
fn build(target: &str) -> Result<()> {
    if !TARGETS.contains(&target) {
        bail!("unsupported target `{target}`; expected one of {TARGETS:?}");
    }
    let mut args: Vec<&str> = if is_windows(target) {
        vec!["xwin", "build"]
    } else {
        vec!["build"]
    };
    args.extend(["--release", "-p", "probe", "--target", target]);
    cargo(&args)?;

    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_owned());
    let exe = exe_name(target);
    let built = Path::new(&target_dir)
        .join(target)
        .join("release")
        .join(exe);
    let out_dir = Path::new(DIST).join(target);
    fs::create_dir_all(&out_dir)?;
    fs::copy(&built, out_dir.join(exe))
        .with_context(|| format!("failed to copy {}", built.display()))?;
    println!("built {}", out_dir.join(exe).display());
    Ok(())
}

/// Archives every built target and writes `dist/SHA256SUMS`.
fn package() -> Result<()> {
    let dist = Path::new(DIST);
    let mut archives = Vec::new();
    for target in TARGETS {
        if !dist.join(target).join(exe_name(target)).is_file() {
            println!("skipping {target}: not built");
            continue;
        }
        let archive = if is_windows(target) {
            let name = format!("faset-probe-{target}.zip");
            let _ = fs::remove_file(dist.join(&name));
            run_cmd("zip", &["-r", "-q", &name, target], Some(dist))?;
            name
        } else {
            let name = format!("faset-probe-{target}.tar.gz");
            run_cmd("tar", &["-czf", &name, target], Some(dist))?;
            name
        };
        archives.push(archive);
    }
    if archives.is_empty() {
        bail!("nothing to package: run `build --target <t>` first");
    }
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
    println!("packaged {} archive(s)", archives.len());
    Ok(())
}

/// Map validation. Stub until the map crates exist (lot 3.x).
fn maps_validate() -> Result<()> {
    println!("maps validate: 0 maps found");
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
    fn build_rejects_unknown_target() {
        assert!(run(&["build", "--target", "riscv64-unknown-none"]).is_err());
        assert!(run(&["build", "--target", "aarch64-apple-darwin"]).is_err());
    }
}
