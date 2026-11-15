//! Developer task runner: `cargo xtask <command>`.
//!
//! All build, test and quality logic lives here so that the host scripts
//! (`dev.ps1`, `dev.sh`) stay thin wrappers around Docker.
#![forbid(unsafe_code)]

use std::process::Command;

use anyhow::{bail, Context, Result};

const USAGE: &str = "usage: cargo xtask <check|test|render-test|maps validate|deny|ci>";

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
        "--",
        "-D",
        "warnings",
    ])
}

/// Unit and integration tests through nextest.
fn test() -> Result<()> {
    cargo(&["nextest", "run", "--workspace", "--no-tests=pass"])
}

/// Headless lavapipe render tests. Stub until lot 0.3 adds the first one.
fn render_test() -> Result<()> {
    println!("render-test: 0 tests (lavapipe probe arrives in lot 0.3)");
    Ok(())
}

/// Map validation. Stub until the map crates exist (lot 3.x).
fn maps_validate() -> Result<()> {
    println!("maps validate: 0 maps found");
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
        assert!(run(&["render-test"]).is_ok());
        assert!(run(&["maps", "validate"]).is_ok());
    }
}
