//! Code-generated low-poly vegetation (lot 6.11): every species of
//! [`faset_mapfmt::Species`] in [`faset_mapfmt::vegetation::VARIANTS`] shape
//! variants and [`LODS`] levels of detail, coloured by a pixel-art palette of
//! our own with one variant per map theme.
//!
//! Nothing is imported: each plant is a [`Recipe`] assembled in code
//! (cylinders, cones, icospheres, ramps) and built by [`build_mesh`], so a
//! plant is deterministic for a species, variant, level of detail and theme.
//! The base of every plant stands at the origin, +Y up.
//!
//! Decisions of lot 6.11:
//! - two levels of detail: LOD 0 up close, LOD 1 (about a fifth of the
//!   triangles) further out; the distances, the fade and the wind sway of each
//!   species are in [`PlantLook`];
//! - themes: `nature` (the default, also used by any theme it does not
//!   know), `snow` (dark conifers with white tips, pale grass) and `desert`
//!   (olive leaves, straw grass, sandstone rocks);
//! - materials: the renderer's pixel patterns (wood, leaves, grass, stone),
//!   so plants match the pixel-art look of the maps.

use faset_mapfmt::Species;

use crate::{build_mesh, BuildError, Mesh, Palette, Part, Pattern, Recipe};

/// Levels of detail of each plant.
pub const LODS: usize = 2;

/// Triangle budget of one plant at LOD 0 and LOD 1.
pub const PLANT_TRIANGLE_BUDGET: [usize; LODS] = [200, 48];

/// How a species is drawn: levels of detail, fade, wind and shadows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantLook {
    /// LOD `k` is drawn up to `lod_distances[k]` metres from the camera; the
    /// last distance is where the plant has faded out.
    pub lod_distances: [f32; LODS],
    /// Wind sway: horizontal offset per squared metre of height above the
    /// base, at a nominal breeze (0: rigid).
    pub sway: f32,
    /// Whether the plant casts sun shadows.
    pub casts_shadow: bool,
}

/// Drawing parameters of a species.
#[must_use]
pub fn look(species: Species) -> PlantLook {
    let (lod_distances, sway, casts_shadow) = match species {
        Species::Oak | Species::Birch => ([45.0, 170.0], 0.006, true),
        Species::Pine => ([45.0, 170.0], 0.004, true),
        Species::Palm => ([45.0, 170.0], 0.01, true),
        Species::Bush => ([25.0, 90.0], 0.03, true),
        Species::Rock => ([30.0, 110.0], 0.0, true),
        Species::Grass => ([16.0, 40.0], 0.3, false),
        Species::Fern => ([14.0, 36.0], 0.45, false),
    };
    PlantLook {
        lod_distances,
        sway,
        casts_shadow,
    }
}

/// Colour names used by the plants.
const COLOURS: [&str; 14] = [
    "bark",
    "bark_birch",
    "leaf",
    "leaf_dark",
    "leaf_light",
    "leaf_tip",
    "palm_leaf",
    "frond",
    "nut",
    "grass_dark",
    "grass_light",
    "rock",
    "rock_dark",
    "snow",
];

/// The vegetation palette of a map theme (unknown themes get `nature`).
#[must_use]
pub fn palette(theme: &str) -> Palette {
    // Pixel-art palette of our own: few, saturated, slightly warm tones.
    let nature: [[u8; 3]; 14] = [
        [104, 70, 44],   // bark
        [214, 208, 192], // bark_birch
        [74, 142, 58],   // leaf
        [42, 96, 46],    // leaf_dark
        [128, 178, 72],  // leaf_light
        [150, 196, 88],  // leaf_tip
        [86, 160, 70],   // palm_leaf
        [64, 132, 62],   // frond
        [120, 84, 40],   // nut
        [58, 116, 44],   // grass_dark
        [146, 196, 84],  // grass_light
        [124, 120, 112], // rock
        [84, 82, 80],    // rock_dark
        [236, 240, 246], // snow
    ];
    let colours = match theme {
        "snow" => [
            [88, 62, 46],
            [222, 218, 206],
            [52, 98, 72],
            [30, 66, 52],
            [96, 140, 104],
            [232, 238, 244],
            [76, 132, 84],
            [60, 104, 74],
            [100, 72, 44],
            [112, 124, 96],
            [196, 204, 170],
            [118, 124, 134],
            [76, 82, 94],
            [244, 248, 252],
        ],
        "desert" => [
            [120, 84, 52],
            [220, 206, 180],
            [124, 138, 64],
            [86, 100, 48],
            [168, 170, 86],
            [186, 182, 98],
            [118, 150, 62],
            [110, 132, 64],
            [140, 96, 48],
            [168, 140, 76],
            [222, 196, 120],
            [196, 150, 104],
            [150, 106, 74],
            [240, 236, 226],
        ],
        _ => nature,
    };
    Palette::new(
        COLOURS
            .iter()
            .zip(colours)
            .map(|(n, c)| ((*n).to_owned(), c)),
    )
}

fn at(x: f32, y: f32, z: f32) -> [f32; 3] {
    [x, y, z]
}

fn cylinder(radius: f32, height: f32, sides: u32, pos: [f32; 3], color: &str) -> Part {
    Part::Cylinder {
        radius,
        height,
        sides,
        at: pos,
        color: color.to_owned(),
        pattern: Pattern::PixelWood,
    }
}

fn cone(radius: f32, height: f32, sides: u32, pos: [f32; 3], color: &str, p: Pattern) -> Part {
    Part::Cone {
        radius,
        height,
        sides,
        at: pos,
        color: color.to_owned(),
        pattern: p,
    }
}

fn sphere(radius: f32, subdivisions: u32, pos: [f32; 3], color: &str, p: Pattern) -> Part {
    Part::Sphere {
        radius,
        subdivisions,
        at: pos,
        color: color.to_owned(),
        pattern: p,
    }
}

fn ramp(size: [f32; 3], pos: [f32; 3], color: &str, p: Pattern) -> Part {
    Part::Ramp {
        size,
        at: pos,
        color: color.to_owned(),
        pattern: p,
    }
}

fn group(height_var: f32, parts: Vec<Part>) -> Part {
    Part::Group {
        at: [0.0; 3],
        rotate_y: 0.0,
        scale: [1.0; 3],
        smooth: false,
        height_var,
        parts,
    }
}

fn scaled(pos: [f32; 3], scale: [f32; 3], parts: Vec<Part>) -> Part {
    Part::Group {
        at: pos,
        rotate_y: 0.0,
        scale,
        smooth: false,
        height_var: 0.0,
        parts,
    }
}

fn jitter(amount: f32, parts: Vec<Part>) -> Part {
    Part::Jitter { amount, parts }
}

fn repeat(count: u32, rotate_y: f32, part: Part) -> Part {
    Part::Repeat {
        count,
        rotate_y,
        offset: [0.0; 3],
        part: Box::new(part),
    }
}

/// The parts of a species at a level of detail (0 or 1).
#[allow(clippy::too_many_lines)]
fn parts(species: Species, lod: usize) -> Vec<Part> {
    use Pattern::{PixelGrass as Grass, PixelLeaves as Leaves, PixelStone as Stone};
    let near = lod == 0;
    match species {
        Species::Oak => vec![group(
            0.12,
            if near {
                vec![
                    cylinder(0.22, 2.0, 6, at(0.0, 1.0, 0.0), "bark"),
                    jitter(
                        0.1,
                        vec![sphere(1.25, 1, at(0.0, 2.85, 0.0), "leaf", Leaves)],
                    ),
                    sphere(0.8, 0, at(0.65, 2.4, 0.3), "leaf_dark", Leaves),
                    sphere(0.7, 0, at(-0.55, 3.35, -0.3), "leaf_light", Leaves),
                ]
            } else {
                vec![
                    cylinder(0.22, 2.0, 4, at(0.0, 1.0, 0.0), "bark"),
                    sphere(1.3, 0, at(0.0, 2.85, 0.0), "leaf", Leaves),
                ]
            },
        )],
        Species::Pine => {
            let (sides, tiers): (u32, &[(f32, f32, f32)]) = if near {
                (7, &[(1.2, 2.2, 2.0), (0.95, 1.9, 3.25), (0.65, 1.6, 4.4)])
            } else {
                (4, &[(1.2, 2.6, 2.3), (0.8, 2.2, 3.9)])
            };
            let cones = tiers
                .iter()
                .map(|&(r, h, y)| cone(r, h, sides, at(0.0, y, 0.0), "leaf_dark", Leaves))
                .collect();
            vec![group(
                0.15,
                vec![
                    cylinder(
                        0.18,
                        1.3,
                        if near { 5 } else { 4 },
                        at(0.0, 0.65, 0.0),
                        "bark",
                    ),
                    Part::Gradient {
                        from: "leaf_dark".into(),
                        to: "leaf_tip".into(),
                        parts: cones,
                    },
                ],
            )]
        }
        Species::Birch => vec![group(
            0.1,
            vec![
                cylinder(
                    0.12,
                    3.0,
                    if near { 5 } else { 4 },
                    at(0.0, 1.5, 0.0),
                    "bark_birch",
                ),
                scaled(
                    at(0.0, 3.5, 0.0),
                    [0.8, 1.35, 0.8],
                    vec![sphere(
                        1.1,
                        u32::from(near),
                        at(0.0, 0.0, 0.0),
                        "leaf_light",
                        Leaves,
                    )],
                ),
            ],
        )],
        Species::Palm => {
            let fronds = if near {
                repeat(
                    7,
                    360.0 / 7.0,
                    ramp([0.36, 0.44, 1.5], at(0.0, 0.0, 0.8), "palm_leaf", Leaves),
                )
            } else {
                repeat(
                    4,
                    90.0,
                    ramp([0.4, 0.44, 1.5], at(0.0, 0.0, 0.8), "palm_leaf", Leaves),
                )
            };
            let mut crown = vec![fronds];
            if near {
                crown.push(repeat(
                    2,
                    180.0,
                    sphere(0.12, 0, at(0.18, -0.25, 0.0), "nut", Pattern::Flat),
                ));
            }
            let trunk = if near {
                Part::Repeat {
                    count: 5,
                    rotate_y: 0.0,
                    offset: [0.08, 0.8, 0.0],
                    part: Box::new(cylinder(0.16, 0.85, 4, at(0.0, 0.425, 0.0), "bark")),
                }
            } else {
                cylinder(0.17, 4.05, 4, at(0.16, 2.025, 0.0), "bark")
            };
            vec![group(
                0.1,
                vec![trunk, scaled(at(0.32, 4.05, 0.0), [1.0; 3], crown)],
            )]
        }
        Species::Bush => vec![group(
            0.15,
            if near {
                vec![
                    jitter(
                        0.06,
                        vec![sphere(0.65, 1, at(0.0, 0.55, 0.0), "leaf_dark", Leaves)],
                    ),
                    sphere(0.45, 0, at(0.42, 0.4, 0.2), "leaf", Leaves),
                    sphere(0.4, 0, at(-0.4, 0.38, -0.25), "leaf", Leaves),
                ]
            } else {
                vec![sphere(0.7, 0, at(0.0, 0.5, 0.0), "leaf_dark", Leaves)]
            },
        )],
        Species::Rock => vec![jitter(
            if near { 0.1 } else { 0.08 },
            vec![scaled(
                at(0.0, 0.25, 0.0),
                [1.0, 0.65, 0.85],
                vec![Part::Gradient {
                    from: "rock_dark".into(),
                    to: "rock".into(),
                    parts: vec![sphere(
                        0.6,
                        u32::from(near),
                        at(0.0, 0.0, 0.0),
                        "rock",
                        Stone,
                    )],
                }],
            )],
        )],
        Species::Grass => {
            let (count, step) = if near { (5, 72.0) } else { (3, 120.0) };
            vec![group(
                0.3,
                vec![Part::Gradient {
                    from: "grass_dark".into(),
                    to: "grass_light".into(),
                    parts: vec![repeat(
                        count,
                        step,
                        cone(0.05, 0.55, 3, at(0.09, 0.275, 0.0), "grass_dark", Grass),
                    )],
                }],
            )]
        }
        Species::Fern => {
            let (count, step) = if near { (6, 60.0) } else { (4, 90.0) };
            vec![group(
                0.2,
                vec![repeat(
                    count,
                    step,
                    ramp([0.14, 0.3, 0.6], at(0.0, 0.15, 0.32), "frond", Leaves),
                )],
            )]
        }
    }
}

/// The recipe of a species at a level of detail (`lod < LODS`).
#[must_use]
pub fn plant_recipe(species: Species, lod: usize) -> Recipe {
    Recipe {
        id: format!("plant_{}_lod{lod}", species.name()),
        tags: vec!["vegetation".to_owned()],
        collision: crate::Collision::None,
        seed: None,
        parts: parts(species, lod.min(LODS - 1)),
    }
}

/// The mesh of one plant: species, shape variant, level of detail and map
/// theme (the palette, see [`palette`]).
///
/// # Errors
/// Only if the generated recipe is invalid (a bug, caught by the tests).
pub fn plant_mesh(
    species: Species,
    variant: u32,
    lod: usize,
    theme: &str,
) -> Result<Mesh, BuildError> {
    let seed = 0x5eed_0000 + u64::from(variant) * 7919 + species.index() as u64 * 104_729;
    build_mesh(&plant_recipe(species, lod), &palette(theme), Some(seed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_mapfmt::vegetation::VARIANTS;

    #[test]
    fn every_plant_builds_within_its_budget() {
        for theme in ["nature", "snow", "desert", "neon"] {
            for species in Species::ALL {
                for variant in 0..VARIANTS {
                    for (lod, budget) in PLANT_TRIANGLE_BUDGET.iter().enumerate() {
                        let mesh = plant_mesh(species, variant, lod, theme)
                            .unwrap_or_else(|e| panic!("{species:?} lod {lod}: {e}"));
                        let n = mesh.triangle_count();
                        assert!(
                            (2..=*budget).contains(&n),
                            "{species:?} lod {lod}: {n} triangles"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lower_detail_has_fewer_triangles() {
        for species in Species::ALL {
            let a = plant_mesh(species, 0, 0, "nature")
                .unwrap()
                .triangle_count();
            let b = plant_mesh(species, 0, 1, "nature")
                .unwrap()
                .triangle_count();
            assert!(b < a, "{species:?}: {b} >= {a}");
            let l = look(species);
            assert!(l.lod_distances[0] < l.lod_distances[1]);
        }
    }

    #[test]
    fn plants_are_deterministic_and_variants_differ() {
        for species in [Species::Oak, Species::Rock, Species::Grass] {
            let a = plant_mesh(species, 1, 0, "nature").unwrap();
            assert_eq!(a, plant_mesh(species, 1, 0, "nature").unwrap());
            assert_ne!(a, plant_mesh(species, 2, 0, "nature").unwrap());
        }
        // Themes recolour without reshaping.
        let n = plant_mesh(Species::Pine, 0, 0, "nature").unwrap();
        let s = plant_mesh(Species::Pine, 0, 0, "snow").unwrap();
        assert_eq!(n.triangle_count(), s.triangle_count());
        assert_ne!(n, s);
    }

    #[test]
    fn plants_stand_on_the_origin_and_match_their_sight_blockers() {
        for species in Species::ALL {
            for variant in 0..VARIANTS {
                let mesh = plant_mesh(species, variant, 0, "nature").unwrap();
                let (lo, hi) = mesh.bounds().unwrap();
                assert!(lo.y > -0.3 && lo.y < 0.3, "{species:?} base at {}", lo.y);
                for c in species.info().sight {
                    // A blocker never sticks out of the plant by more than
                    // the height variation.
                    assert!(c.top <= hi.y * 1.2 + 0.05, "{species:?} top");
                    let half = (hi.x - lo.x).max(hi.z - lo.z) * 0.5;
                    assert!(c.radius <= half + 0.05, "{species:?} radius");
                }
            }
        }
    }
}
