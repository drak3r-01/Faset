//! Lot 6.12 acceptance: the landscape beyond a map is only scenery. The
//! playable boundary still contains the players: everything a player can
//! reach is inside the declared area, and past its edge there is either a
//! solid wall or a kill volume under the feet, with no visible wall needed.

use std::path::Path;

use faset_client::load_map;
use faset_mapfmt::{grid_to_metres, Border, Zone, ZoneKind};
use faset_render::glam::Vec3;

const MAPS: [&str; 4] = ["driftwood-cove", "red-mesa", "pine-hollow", "rust-harbor"];

fn load(name: &str) -> faset_client::LoadedMap {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/maps")
        .join(format!("{name}.ron"));
    load_map(&std::fs::read(path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Footprints `(min_x, min_z, max_x, max_z)` of the kill volumes, metres.
fn kill_footprints(map: &faset_mapfmt::MapFile) -> Vec<[f32; 4]> {
    map.zones
        .iter()
        .filter(|Zone::Box { kind, .. }| matches!(kind, ZoneKind::KillVolume))
        .map(|Zone::Box { min, max, .. }| {
            let (lo, hi) = (grid_to_metres(*min), grid_to_metres(*max));
            [lo.x, lo.z, hi.x, hi.z]
        })
        .collect()
}

fn in_footprint(f: &[f32; 4], x: f32, z: f32) -> bool {
    (f[0]..=f[2]).contains(&x) && (f[1]..=f[3]).contains(&z)
}

#[test]
fn every_shipped_map_has_surroundings_and_keeps_its_players_inside() {
    for name in MAPS {
        let loaded = load(name);
        let map = &loaded.map;
        let s = map
            .surroundings
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: no surroundings"));
        let land = loaded
            .landscape
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: no landscape"));
        assert!(!land.indices.is_empty(), "{name}");

        // Everything the players start from lies inside the area.
        for sp in &map.spawns {
            assert!(s.contains(sp.at[0], sp.at[2]), "{name}: spawn {:?}", sp.at);
        }
        for p in &map.pickups {
            assert!(s.contains(p.at[0], p.at[2]), "{name}: pickup {:?}", p.at);
        }

        // One kill volume spans the whole area: the ground past the built
        // edge is a drop, not a place to walk to.
        let kills = kill_footprints(map);
        assert!(
            kills.iter().any(|f| f[0] <= s.area[0]
                && f[1] <= s.area[1]
                && f[2] >= s.area[2]
                && f[3] >= s.area[3]),
            "{name}: no kill volume covers the area {:?}",
            s.area
        );

        // Walk the edge of the area: from the sky down, the ray
        // meets a solid (wall, cliff, rock) or the point is over a kill
        // volume.
        let [x0, z0, x1, z1] = s.area;
        let step = 2.0;
        let mut points = Vec::new();
        let mut t = 0.0;
        while x0 + t <= x1 {
            points.push((x0 + t, z0));
            points.push((x0 + t, z1));
            t += step;
        }
        t = 0.0;
        while z0 + t <= z1 {
            points.push((x0, z0 + t));
            points.push((x1, z0 + t));
            t += step;
        }
        for (x, z) in points {
            let over_kill = kills.iter().any(|f| in_footprint(f, x, z));
            let solid = loaded
                .collision
                .ray_cast(Vec3::new(x, 60.0, z), -Vec3::Y, 200.0)
                .is_some();
            assert!(
                over_kill || solid,
                "{name}: ({x}, {z}) leads out of the map"
            );
        }
    }
}

#[test]
fn the_maps_have_varied_borders() {
    let mut sides = std::collections::HashSet::new();
    for name in MAPS {
        let loaded = load(name);
        let s = loaded.map.surroundings.unwrap();
        let own: std::collections::HashSet<Border> = s.borders().into_iter().collect();
        sides.extend(own.iter().copied());
        assert!(!own.is_empty());
    }
    // Sea, forest, cliff and rock at least appear across the maps.
    for b in [Border::Sea, Border::Forest, Border::Cliff, Border::Rocks] {
        assert!(sides.contains(&b), "{b:?} is not used by any map");
    }
}

#[test]
fn landscape_is_cheap() {
    for name in MAPS {
        let land = load(name).landscape.unwrap();
        assert!(
            land.vertices.len() <= 4_000,
            "{name}: {}",
            land.vertices.len()
        );
        assert!(land.indices.len() / 3 <= 8_000, "{name}");
        assert!(
            land.plants.len() <= faset_client::landscape::MAX_TREES,
            "{name}"
        );
    }
}
