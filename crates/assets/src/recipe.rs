use serde::{Deserialize, Serialize};

use crate::RecipeError;

/// Declared collision shape of a prop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Collision {
    None,
    #[default]
    Box,
    ConvexHull,
    Compound,
}

/// Procedural surface pattern. The discriminant is the material id written
/// into every vertex (ids match the render crate's built-in material table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pattern {
    #[default]
    Flat = 0,
    Gradient = 1,
    Noise = 2,
    Checker = 3,
    Stripes = 4,
    Grid = 5,
    Brick = 6,
    Planks = 7,
    Tiles = 8,
    Ripples = 9,
    Water = 10,
    Lava = 11,
    Emissive = 12,
    Hologram = 13,
    PixelBrick = 14,
    PixelWood = 15,
    PixelGrass = 16,
    PixelSand = 17,
    PixelStone = 18,
    PixelConcrete = 19,
    PixelMetal = 20,
    PixelWater = 21,
    PixelTile = 22,
    PixelDirt = 23,
    PixelSnow = 24,
    PixelCrate = 25,
    PixelHazard = 26,
    PixelLeaves = 27,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

fn zero3() -> [f32; 3] {
    [0.0; 3]
}

fn one3() -> [f32; 3] {
    [1.0; 3]
}

/// One building block. Primitives are centred on their own origin (then
/// moved by `at`); operations take sub-parts.
///
/// Angles are degrees. Counter-clockwise faces point outward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Part {
    /// Axis-aligned box.
    Box {
        size: [f32; 3],
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Wedge: full height at -Z, sloping down to 0 at +Z.
    Ramp {
        size: [f32; 3],
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Cylinder around Y with `sides` facets (3 to 64).
    Cylinder {
        radius: f32,
        height: f32,
        sides: u32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Cone around Y, apex up.
    Cone {
        radius: f32,
        height: f32,
        sides: u32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Icosphere with `subdivisions` 0 to 3 (20 * 4^n triangles).
    Sphere {
        radius: f32,
        subdivisions: u32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Triangular prism (tent / roof): ridge along Z, apex at +Y.
    Prism {
        size: [f32; 3],
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Torus around Y.
    Torus {
        major_radius: f32,
        minor_radius: f32,
        major_segments: u32,
        minor_segments: u32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Simple polygon in the XZ plane (`[x, z]` points) extruded along Y,
    /// centred vertically.
    Extrude {
        polygon: Vec<[f32; 2]>,
        height: f32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Surface of revolution around Y. `profile` holds `[radius, y]` points;
    /// the outward side of each segment is `(dy, -dr)` (trace a closed solid
    /// with the outside on the right).
    Revolve {
        profile: Vec<[f32; 2]>,
        sides: u32,
        #[serde(default = "zero3")]
        at: [f32; 3],
        color: String,
        #[serde(default)]
        pattern: Pattern,
    },
    /// Transform and merge: scale, then rotate about Y, then translate.
    /// `smooth` averages normals of coincident vertices (smoothing group);
    /// `height_var` randomises the Y scale by +-`height_var` from the seed.
    Group {
        #[serde(default = "zero3")]
        at: [f32; 3],
        #[serde(default)]
        rotate_y: f32,
        #[serde(default = "one3")]
        scale: [f32; 3],
        #[serde(default)]
        smooth: bool,
        #[serde(default)]
        height_var: f32,
        parts: Vec<Part>,
    },
    /// Adds a copy of `parts` mirrored across the plane normal to `axis`.
    Mirror { axis: Axis, parts: Vec<Part> },
    /// `count` copies; copy `i` is turned `i * rotate_y` degrees about Y,
    /// then moved by `i * offset`.
    Repeat {
        count: u32,
        #[serde(default)]
        rotate_y: f32,
        #[serde(default = "zero3")]
        offset: [f32; 3],
        part: Box<Part>,
    },
    /// Per-vertex colours: blends `from` (lowest vertex) to `to` (highest).
    Gradient {
        from: String,
        to: String,
        parts: Vec<Part>,
    },
    /// Seeded random displacement of every vertex by up to `amount` metres
    /// per axis (rock shape); coincident vertices move together.
    Jitter { amount: f32, parts: Vec<Part> },
}

/// A prop: parts, tags and a declared collision shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Prop")]
pub struct Recipe {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub collision: Collision,
    /// Default seed of variants (tree height, rock shape).
    #[serde(default)]
    pub seed: Option<u64>,
    pub parts: Vec<Part>,
}

impl Recipe {
    pub fn from_ron(text: &str) -> Result<Self, RecipeError> {
        Ok(ron::from_str(text)?)
    }

    pub fn to_ron(&self) -> Result<String, RecipeError> {
        Ok(ron::ser::to_string_pretty(
            self,
            ron::ser::PrettyConfig::default(),
        )?)
    }
}
