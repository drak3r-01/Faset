//! The dedicated server must not link graphics or audio crates.

use std::process::Command;

#[test]
fn dependency_tree_has_no_graphics_or_audio() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "tree",
            "-p",
            "dedicated",
            "--prefix",
            "none",
            "--edges",
            "normal,build",
        ])
        .output()
        .expect("cargo tree runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout);
    for forbidden in [
        "ash",
        "winit",
        "kira",
        "faset-render",
        "faset-client",
        "cpal",
    ] {
        assert!(
            !tree.lines().any(|l| l.split(' ').next() == Some(forbidden)),
            "`{forbidden}` is in the dedicated dependency tree"
        );
    }
}
