//! Lot 5.5 acceptance: the trial report and the design guide.

use std::path::Path;
use std::process::Command;

use map_tool::trial::{run_trial, TrialConfig};

fn root(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn trial(seed: &str, out: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_map-tool"))
        .args(["trial", &root("../../assets/maps/example.ron")])
        .args(["--seed", seed, "--out"])
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn trial_report_has_the_five_metrics() {
    let out = std::env::temp_dir().join(format!("trial-{}.json", std::process::id()));
    trial("7", &out);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(v["schema_version"], 1);
    for field in [
        "elimination_zones",
        "spawn_deaths",
        "mean_engagement_distance_m",
        "unvisited_zones",
        "team_balance",
    ] {
        assert!(!v[field].is_null(), "missing {field}");
    }
    assert_eq!(v["bots"], 16);
    assert_eq!(v["match_seconds"], 300.0);
    assert!(v["total_kills"].as_u64().unwrap() > 0, "bots never killed");
    assert_eq!(v["team_balance"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_file(out);
}

#[test]
fn same_seed_gives_a_byte_identical_report() {
    let bytes = std::fs::read(root("../../assets/maps/example.ron")).unwrap();
    let map = faset_mapfmt::parse(&bytes).unwrap();
    let cfg = TrialConfig {
        seconds: 40.0,
        ..TrialConfig::new(3)
    };
    let (a, b) = (
        run_trial(&map, &cfg).to_json(),
        run_trial(&map, &cfg).to_json(),
    );
    assert_eq!(a, b);
}

#[test]
fn design_guide_contains_every_dimension() {
    let guide = std::fs::read_to_string(root("../../docs/map-design-guide.md")).unwrap();
    for value in [
        "at least 3 m",
        "2 m × 2.5 m",
        "1.0 m and 2.0 m",
        "0.45 m",
        "1.0 m",
        "4 m",
        "60 m",
        "map-tool validate",
        "map-tool trial",
    ] {
        assert!(guide.contains(value), "guide misses {value:?}");
    }
}
