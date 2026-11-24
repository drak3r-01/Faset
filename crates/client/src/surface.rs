//! What a shot hits (lot 6.9): the kind of surface under an impact, which
//! picks the particles, the decal and the sound.
//!
//! Decision: the surface kind is looked up from the map file, not from the
//! renderer. Each collision volume of [`MapCollision`](faset_sim::MapCollision)
//! is one brush of the expanded map (then one volume per prop, in order), so
//! a ray hit's `volume` index finds the brush and its material name; the name
//! is classified by [`Surface::from_material`]. A map whose volume count does
//! not match its brushes and props (a degenerate brush was dropped) falls back
//! to [`Surface::Concrete`] everywhere rather than guess. Water is a height:
//! a ray that crosses the sea level before it reaches a volume splashes.

use faset_mapfmt::geometry::{terrain_material, TERRAIN_MATERIALS};
use faset_mapfmt::terrain::PAINTS;
use faset_mapfmt::{Brush, MapFile};

/// The kind of surface an impact lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Surface {
    Metal,
    #[default]
    Concrete,
    Stone,
    Sand,
    Wood,
    Water,
    Snow,
}

impl Surface {
    /// Every kind, for tables and tests.
    pub const ALL: [Surface; 7] = [
        Surface::Metal,
        Surface::Concrete,
        Surface::Stone,
        Surface::Sand,
        Surface::Wood,
        Surface::Water,
        Surface::Snow,
    ];

    /// The kind of a map material name (`metal_orange`, `wood_dark`, ...).
    /// An unknown name is concrete.
    #[must_use]
    pub fn from_material(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| name.contains(n));
        if has(&["snow", "ice"]) {
            Self::Snow
        } else if has(&["water"]) {
            Self::Water
        } else if has(&["metal", "container", "hazard", "lamp"]) {
            Self::Metal
        } else if has(&["wood", "crate", "plank", "leaves"]) {
            Self::Wood
        } else if has(&["sandstone", "stone", "basalt", "rock"]) {
            Self::Stone
        } else if has(&["sand", "dirt", "clay", "grass", "mud"]) {
            Self::Sand
        } else {
            Self::Concrete
        }
    }

    /// Id used in sound recipe names (`impact_<name>`, `footstep_<name>`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Concrete => "concrete",
            Self::Stone => "stone",
            Self::Sand => "sand",
            Self::Wood => "wood",
            Self::Water => "water",
            Self::Snow => "snow",
        }
    }
}

/// The surface kind of every collision volume of a map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceMap {
    volumes: Vec<Surface>,
    water_height: Option<f32>,
}

fn brush_material(brush: &Brush) -> &str {
    match brush {
        Brush::Box { material, .. }
        | Brush::Wedge { material, .. }
        | Brush::Cylinder { material, .. }
        | Brush::Prism { material, .. }
        | Brush::Stairs { material, .. } => material,
    }
}

impl SurfaceMap {
    /// The surfaces of an expanded map whose collision has `volume_count`
    /// volumes. Props are wooden (their collision is a placeholder crate).
    #[must_use]
    pub fn from_map(map: &MapFile, volume_count: usize) -> Self {
        // Terrain triangles come last (lot 6.10), one volume each: the ground
        // of their first corner (its paint, or the palette's ground).
        let terrain: Vec<Surface> = map
            .terrain
            .iter()
            .flat_map(|t| {
                t.triangles().map(move |tri| {
                    let (i, j) = tri[0];
                    let id = terrain_material(&map.theme.palette, t.paint_at(i, j));
                    let ground = TERRAIN_MATERIALS.iter().position(|m| *m == id);
                    Surface::from_material(PAINTS[ground.map_or(1, |k| k + 1)])
                })
            })
            .collect();
        let expected = map.brushes.len() + map.props.len() + terrain.len();
        let volumes = if volume_count == expected {
            map.brushes
                .iter()
                .map(|b| Surface::from_material(brush_material(b)))
                .chain(map.props.iter().map(|_| Surface::Wood))
                .chain(terrain)
                .collect()
        } else {
            tracing::warn!(
                volume_count,
                expected,
                "collision volumes do not match the map: impacts use the default surface"
            );
            Vec::new()
        };
        Self {
            volumes,
            water_height: None,
        }
    }

    /// The same map with a sea level (metres), or none.
    #[must_use]
    pub fn with_water(mut self, height: Option<f32>) -> Self {
        self.water_height = height;
        self
    }

    /// The sea level, if the map has water.
    #[must_use]
    pub fn water_height(&self) -> Option<f32> {
        self.water_height
    }

    /// The surface of collision volume `volume`.
    #[must_use]
    pub fn of_volume(&self, volume: usize) -> Surface {
        self.volumes.get(volume).copied().unwrap_or_default()
    }

    /// Where a ray from `origin` along unit `dir` meets the sea level before
    /// `max_distance`, if it does: the distance along the ray.
    #[must_use]
    pub fn water_hit(
        &self,
        origin: faset_core::glam::Vec3,
        dir: faset_core::glam::Vec3,
        max_distance: f32,
    ) -> Option<f32> {
        let level = self.water_height?;
        if origin.y <= level || dir.y >= -1e-4 {
            return None;
        }
        let t = (level - origin.y) / dir.y;
        (0.0..max_distance).contains(&t).then_some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::glam::Vec3;

    #[test]
    fn every_material_name_of_the_maps_has_a_kind() {
        let expect = [
            ("metal", Surface::Metal),
            ("metal_orange", Surface::Metal),
            ("container_red", Surface::Metal),
            ("hazard", Surface::Metal),
            ("concrete", Surface::Concrete),
            ("asphalt", Surface::Concrete),
            ("brick_slate", Surface::Concrete),
            ("tiles_teal", Surface::Concrete),
            ("stone", Surface::Stone),
            ("mossy_stone", Surface::Stone),
            ("basalt", Surface::Stone),
            ("sandstone", Surface::Stone),
            ("sand", Surface::Sand),
            ("sand_white", Surface::Sand),
            ("dirt", Surface::Sand),
            ("clay", Surface::Sand),
            ("grass", Surface::Sand),
            ("wood", Surface::Wood),
            ("wood_dark", Surface::Wood),
            ("crate", Surface::Wood),
            ("palm_leaves", Surface::Wood),
            ("leaves", Surface::Wood),
            ("snow", Surface::Snow),
            ("ice", Surface::Snow),
            ("lagoon_water", Surface::Water),
        ];
        for (name, kind) in expect {
            assert_eq!(Surface::from_material(name), kind, "{name}");
        }
        assert_eq!(Surface::from_material("mystery"), Surface::Concrete);
    }

    #[test]
    fn a_ray_hit_finds_its_brush_then_its_prop() {
        let text = include_str!("../../../assets/testdata/example.ron");
        let map = faset_mapfmt::expand(&faset_mapfmt::parse(text.as_bytes()).unwrap());
        let count = map.brushes.len() + map.props.len();
        let surfaces = SurfaceMap::from_map(&map, count);
        for (i, brush) in map.brushes.iter().enumerate() {
            assert_eq!(
                surfaces.of_volume(i),
                Surface::from_material(brush_material(brush))
            );
        }
        for i in 0..map.props.len() {
            assert_eq!(surfaces.of_volume(map.brushes.len() + i), Surface::Wood);
        }
        // Out of range or a mismatching count: the default.
        assert_eq!(surfaces.of_volume(count + 5), Surface::Concrete);
        let off = SurfaceMap::from_map(&map, count + 1);
        assert!((0..count).all(|i| off.of_volume(i) == Surface::Concrete));
    }

    #[test]
    fn terrain_triangles_take_the_ground_of_their_paint() {
        let text = include_str!("../../../assets/maps/pine-hollow.ron");
        let mut map = faset_mapfmt::expand(&faset_mapfmt::parse(text.as_bytes()).unwrap());
        let col = faset_sim::MapCollision::build(&map);
        let surfaces = SurfaceMap::from_map(&map, col.volume_count());
        let first = map.brushes.len() + map.props.len();
        // Pine Hollow's palette is snow.
        assert_eq!(surfaces.of_volume(first), Surface::Snow);
        assert_eq!(surfaces.of_volume(col.volume_count() - 1), Surface::Snow);
        // Paint 3 (rock) on every sample.
        let t = map.terrain.as_mut().unwrap();
        t.paint = vec![3; t.heights.len()];
        let rock = SurfaceMap::from_map(&map, col.volume_count());
        assert_eq!(rock.of_volume(first), Surface::Stone);
    }

    #[test]
    fn a_ray_crossing_the_sea_before_the_ground_splashes() {
        let s = SurfaceMap::default().with_water(Some(1.0));
        let d = Vec3::new(0.0, -1.0, 0.0);
        assert_eq!(s.water_hit(Vec3::new(0.0, 3.0, 0.0), d, 10.0), Some(2.0));
        assert_eq!(s.water_hit(Vec3::new(0.0, 3.0, 0.0), d, 1.0), None);
        assert_eq!(s.water_hit(Vec3::new(0.0, 0.5, 0.0), d, 10.0), None);
        assert_eq!(s.water_hit(Vec3::new(0.0, 3.0, 0.0), Vec3::Y, 10.0), None);
        assert_eq!(
            SurfaceMap::default().water_hit(Vec3::new(0.0, 3.0, 0.0), d, 10.0),
            None
        );
    }
}
