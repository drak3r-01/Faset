//! Map validator: checks a map against the nine rules of lot 3.3, produces a
//! JSON report and renders top and perspective views.
//!
//! [`validate`] works on a parsed [`MapFile`]; [`validate_bytes`] starts from
//! the file bytes and also covers rules 1 and 2. Every violation names the
//! rule, the map element (`spawn[3]` and so on, indexed in the expanded map:
//! prefabs and symmetry applied) and a precise message.
//!
//! Decisions of lot 3.3 (see `docs/lots/3.3-map-tool.md`):
//! - rule 5: a match has 16 players, so each team needs at least 8 spawns
//!   (`ceil(max / 2)`, `max` = the smaller of the map's declared maximum and
//!   16); `Any` spawns count for both teams. A map with no team spawns needs
//!   `max` `Any` spawns;
//! - rule 6: own walk search, see [`nav`];
//! - rule 9: major pickups are Health and Armor (the format has no damage
//!   pickup); a team's figure is the mean walking distance from its nearest
//!   spawn to each objective, and to each major pickup;
//! - rule 8: the footprint of a brush is its bounding rectangle.
#![forbid(unsafe_code)]

pub mod nav;
mod report;
mod settings;
pub mod trial;
pub mod view;

use std::collections::HashSet;

use faset_core::glam::Vec3;
use faset_mapfmt::geometry::{brush_hull_points, build_meshes};
use faset_mapfmt::{
    fingerprint, grid_to_metres, to_ron, MapError, MapFile, PickupKind, Team, Zone, ZoneKind,
    MAX_MAP_BYTES,
};
use faset_sim::map_collision::PLAYER_HEIGHT;
use faset_sim::MapCollision;

pub use report::{Issue, Report, Stats, REPORT_VERSION};
pub use settings::Settings;

/// Eye height of a standing player above the feet, in metres.
const EYE_HEIGHT_M: f32 = 1.6;

/// Validates the bytes of a map file: rules 1 and 2, then all the others.
#[must_use]
pub fn validate_bytes(bytes: &[u8]) -> Report {
    validate_bytes_with(bytes, &Settings::default())
}

/// [`validate_bytes`] with explicit thresholds.
#[must_use]
pub fn validate_bytes_with(bytes: &[u8], settings: &Settings) -> Report {
    match faset_mapfmt::parse(bytes) {
        Ok(map) => validate_with(&map, settings),
        Err(e) => {
            let rule = if matches!(e, MapError::TooLarge { .. }) {
                2
            } else {
                1
            };
            let mut report = Report::new(None);
            report.error(rule, "file", e.to_string());
            report
        }
    }
}

/// Validates a parsed map against every rule.
#[must_use]
pub fn validate(map: &MapFile) -> Report {
    validate_with(map, &Settings::default())
}

/// [`validate`] with explicit thresholds.
#[must_use]
pub fn validate_with(map: &MapFile, settings: &Settings) -> Report {
    let mut report = Report::new(Some(hex(&fingerprint(map))));
    // Rule 1 holds for any map that parsed; rule 2 is checked on the
    // canonical text the id is computed from.
    let size = to_ron(map).len();
    if size > MAX_MAP_BYTES {
        report.error(
            2,
            "file",
            format!("map is {size} bytes, limit is {MAX_MAP_BYTES}"),
        );
    }
    let map = faset_mapfmt::expand(map);
    let col = MapCollision::build(&map);

    check_triangles(&map, settings, &mut report);
    let standing = check_spawns(&map, &col, settings, &mut report);
    check_spawn_counts(&map, settings, &mut report);
    check_kill_volume(&map, &mut report);
    check_sight(&map, &col, settings, &mut report);
    check_walk_and_balance(&map, &col, &standing, settings, &mut report);
    stats(&map, &col, &mut report);
    report
}

fn hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn feet(at: [f32; 3]) -> Vec3 {
    Vec3::from_array(at)
}

/// Rule 3.
#[allow(clippy::cast_possible_truncation)]
fn check_triangles(map: &MapFile, settings: &Settings, report: &mut Report) {
    let tris: usize = build_meshes(map).iter().map(|c| c.indices.len() / 3).sum();
    report.stats.triangles = tris as u32;
    if tris > settings.max_triangles as usize {
        report.error(
            3,
            "map",
            format!(
                "{tris} triangles, budget is {} for the whole map",
                settings.max_triangles
            ),
        );
    }
}

/// Rule 4. Returns the indices of the spawns that stand on a floor.
fn check_spawns(
    map: &MapFile,
    col: &MapCollision,
    settings: &Settings,
    report: &mut Report,
) -> Vec<usize> {
    let mut ok = Vec::new();
    for (i, s) in map.spawns.iter().enumerate() {
        let name = format!("spawn[{i}]");
        let f = feet(s.at);
        let body = f + Vec3::Y * (PLAYER_HEIGHT * 0.5 + 0.01);
        if col.overlaps_capsule(body, PLAYER_HEIGHT) {
            report.error(4, name, format!("spawn at {:?} is inside a solid", s.at));
            continue;
        }
        match col.ray_cast(f + Vec3::Y * 0.5, Vec3::NEG_Y, 1.0) {
            Some(d) if (d - 0.5).abs() <= 0.1 => {}
            Some(d) => {
                report.error(
                    4,
                    name,
                    format!("spawn floats {:.2} m above the floor", d - 0.5),
                );
                continue;
            }
            None => {
                report.error(4, name, "no floor under the spawn");
                continue;
            }
        }
        let room = settings.spawn_free_height_m;
        if let Some(d) = col.ray_cast(f + Vec3::Y * 0.01, Vec3::Y, room) {
            report.error(
                4,
                name,
                format!(
                    "only {:.2} m of free height above the spawn, {room} m needed",
                    d
                ),
            );
            continue;
        }
        ok.push(i);
    }
    ok
}

/// Rule 5.
fn check_spawn_counts(map: &MapFile, settings: &Settings, report: &mut Report) {
    let max = map.meta.players.1.min(settings.match_players);
    let count = |t: Team| map.spawns.iter().filter(|s| s.team == t).count();
    let (a, b, any) = (count(Team::A), count(Team::B), count(Team::Any));
    if a + b == 0 {
        if any < usize::from(max) {
            report.error(
                5,
                "spawns",
                format!("{any} spawns for {max} players, at least {max} needed"),
            );
        }
        return;
    }
    let need = usize::from(max).div_ceil(2);
    for (team, n) in [("A", a), ("B", b)] {
        if n + any < need {
            report.error(
                5,
                format!("team {team}"),
                format!(
                    "{} spawns for {need} players per team ({max} players), at least {need} needed",
                    n + any
                ),
            );
        }
    }
}

/// Rule 7.
fn check_sight(map: &MapFile, col: &MapCollision, settings: &Settings, report: &mut Report) {
    let eye = |i: usize| feet(map.spawns[i].at) + Vec3::Y * EYE_HEIGHT_M;
    for i in 0..map.spawns.len() {
        for j in i + 1..map.spawns.len() {
            let (a, b) = (&map.spawns[i], &map.spawns[j]);
            let enemies = a.team == Team::Any || b.team == Team::Any || a.team != b.team;
            if !enemies {
                continue;
            }
            let dist = (feet(a.at) - feet(b.at)).length();
            if dist >= settings.enemy_sight_radius_m {
                continue;
            }
            let (ea, eb) = (eye(i), eye(j));
            if col.ray_cast(ea, eb - ea, dist - 0.01).is_none() {
                report.error(
                    7,
                    format!("spawn[{i}]"),
                    format!(
                        "in direct line of sight of enemy spawn[{j}], {dist:.1} m away (limit {} m)",
                        settings.enemy_sight_radius_m
                    ),
                );
            }
        }
    }
}

/// Bounding rectangle of the brushes' footprints, per brush, and the lowest
/// solid point.
fn footprints(map: &MapFile) -> (Vec<(Vec3, Vec3)>, f32) {
    let mut boxes = Vec::new();
    let mut lowest = f32::INFINITY;
    for b in &map.brushes {
        let pts = brush_hull_points(b);
        if pts.is_empty() {
            continue;
        }
        let lo = pts
            .iter()
            .fold(Vec3::splat(f32::INFINITY), |m, p| m.min(*p));
        let hi = pts
            .iter()
            .fold(Vec3::splat(f32::NEG_INFINITY), |m, p| m.max(*p));
        lowest = lowest.min(lo.y);
        boxes.push((lo, hi));
    }
    (boxes, lowest)
}

/// Rule 8.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn check_kill_volume(map: &MapFile, report: &mut Report) {
    let (solids, lowest) = footprints(map);
    if solids.is_empty() {
        return;
    }
    let kills: Vec<(Vec3, Vec3)> = map
        .zones
        .iter()
        .filter_map(|Zone::Box { min, max, kind }| match kind {
            ZoneKind::KillVolume => Some((grid_to_metres(*min), grid_to_metres(*max))),
            _ => None,
        })
        .collect();
    if kills.is_empty() {
        report.error(8, "map", "no KillVolume zone under the map");
        return;
    }
    let reaching: Vec<_> = kills.iter().filter(|(lo, _)| lo.y <= lowest).collect();
    if reaching.is_empty() {
        report.error(
            8,
            "map",
            format!("no KillVolume reaches the lowest solid (y = {lowest} m)"),
        );
        return;
    }
    let mut cells = HashSet::new();
    for (lo, hi) in &solids {
        for ix in (lo.x * 2.0).floor() as i32..(hi.x * 2.0).ceil() as i32 {
            for iz in (lo.z * 2.0).floor() as i32..(hi.z * 2.0).ceil() as i32 {
                cells.insert((ix, iz));
            }
        }
    }
    let mut uncovered: Vec<(i32, i32)> = cells
        .into_iter()
        .filter(|&(ix, iz)| {
            let (x, z) = ((ix as f32 + 0.5) * 0.5, (iz as f32 + 0.5) * 0.5);
            !reaching
                .iter()
                .any(|(lo, hi)| x >= lo.x && x <= hi.x && z >= lo.z && z <= hi.z)
        })
        .collect();
    if !uncovered.is_empty() {
        uncovered.sort_unstable();
        let (ix, iz) = uncovered[0];
        report.error(
            8,
            "map",
            format!(
                "{} cells of 0.5 m under the map are not covered by a KillVolume, first at x = {} m, z = {} m",
                uncovered.len(),
                ix as f32 * 0.5,
                iz as f32 * 0.5
            ),
        );
    }
}

/// Rules 6 and 9, and the walk statistics.
#[allow(clippy::cast_precision_loss)]
fn check_walk_and_balance(
    map: &MapFile,
    col: &MapCollision,
    standing: &[usize],
    settings: &Settings,
    report: &mut Report,
) {
    let Some(&first) = standing.first() else {
        return;
    };
    let origin = feet(map.spawns[first].at);
    let reach = nav::reach(col, &[origin], settings.step_m);
    report.stats.playable_surface_m2 = reach.nodes.len() as f32 * nav::CELL_M * nav::CELL_M;
    report.stats.cover_density = reach.bordered as f32 / reach.nodes.len().max(1) as f32;

    for &i in standing {
        if i != first && reach.distance_to(feet(map.spawns[i].at)).is_none() {
            report.error(
                6,
                format!("spawn[{i}]"),
                format!("not reachable on foot from spawn[{first}]"),
            );
        }
    }
    for (i, p) in map.pickups.iter().enumerate() {
        if reach.distance_to(feet(p.at)).is_none() {
            report.error(
                6,
                format!("pickup[{i}]"),
                format!(
                    "{:?} pickup not reachable on foot from spawn[{first}]",
                    p.kind
                ),
            );
        }
    }

    // Rule 9.
    let team_sources = |t: Team| -> Vec<Vec3> {
        standing
            .iter()
            .filter(|&&i| map.spawns[i].team == t)
            .map(|&i| feet(map.spawns[i].at))
            .collect()
    };
    let (sa, sb) = (team_sources(Team::A), team_sources(Team::B));
    if sa.is_empty() || sb.is_empty() {
        return;
    }
    let objectives: Vec<Vec3> = map
        .zones
        .iter()
        .filter_map(|Zone::Box { min, max, kind }| match kind {
            ZoneKind::Objective { .. } => {
                let (lo, hi) = (grid_to_metres(*min), grid_to_metres(*max));
                Some(Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5))
            }
            _ => None,
        })
        .collect();
    let majors: Vec<Vec3> = map
        .pickups
        .iter()
        .filter(|p| matches!(p.kind, PickupKind::Health | PickupKind::Armor))
        .map(|p| feet(p.at))
        .collect();
    if objectives.is_empty() && majors.is_empty() {
        report.warn(
            9,
            "map",
            "no objective or major pickup: balance not checked",
        );
        return;
    }
    let ra = nav::reach(col, &sa, settings.step_m);
    let rb = nav::reach(col, &sb, settings.step_m);
    for (what, targets) in [("objectives", objectives), ("major pickups", majors)] {
        let mean = |r: &nav::Reach| -> Option<f32> {
            let d: Vec<f32> = targets.iter().filter_map(|t| r.distance_to(*t)).collect();
            (!d.is_empty()).then(|| d.iter().sum::<f32>() / d.len() as f32)
        };
        let (Some(a), Some(b)) = (mean(&ra), mean(&rb)) else {
            continue;
        };
        let gap = (a - b).abs() / a.max(b).max(f32::EPSILON);
        if gap >= settings.balance_tolerance {
            report.error(
                9,
                what,
                format!(
                    "mean walking distance is {a:.1} m for team A and {b:.1} m for team B, a gap of {:.1} % (limit {:.0} %)",
                    gap * 100.0,
                    settings.balance_tolerance * 100.0
                ),
            );
        }
    }
}

/// Informative statistics that need no walk search.
#[allow(clippy::cast_possible_truncation)]
fn stats(map: &MapFile, col: &MapCollision, report: &mut Report) {
    report.stats.spawns = map.spawns.len() as u32;
    report.stats.pickups = map.pickups.len() as u32;
    let mut points: Vec<Vec3> = map
        .spawns
        .iter()
        .map(|s| feet(s.at) + Vec3::Y * EYE_HEIGHT_M)
        .collect();
    points.extend(map.pickups.iter().map(|p| feet(p.at) + Vec3::Y));
    let mut longest = 0.0f32;
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            let d = (*b - *a).length();
            if d > longest && col.ray_cast(*a, *b - *a, d - 0.01).is_none() {
                longest = d;
            }
        }
    }
    report.stats.longest_line_of_sight_m = longest;
}
