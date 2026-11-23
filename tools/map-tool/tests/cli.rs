//! The command line: exit codes, messages and the JSON report file.

use std::path::Path;
use std::process::Command;

fn tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_map-tool"))
}

fn root(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn example_map_exits_zero_and_writes_a_report() {
    let out = std::env::temp_dir().join(format!("map-tool-cli-{}.json", std::process::id()));
    let status = tool()
        .args([
            "validate",
            &root("../../assets/testdata/example.ron"),
            "--report",
        ])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(v["map_id"].as_str().unwrap().len(), 64);
    assert_eq!(v["errors"].as_array().unwrap().len(), 0);
    let _ = std::fs::remove_file(out);
}

#[test]
fn spawn_in_a_wall_exits_non_zero_naming_spawn_and_rule() {
    let out = tool()
        .args(["validate", &root("fixtures/spawn_in_wall.ron")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("rule 4 spawn[0]"), "{text}");
}

#[test]
fn usage_errors_exit_two() {
    assert_eq!(tool().arg("nope").status().unwrap().code(), Some(2));
    assert_eq!(tool().arg("validate").status().unwrap().code(), Some(2));
}
