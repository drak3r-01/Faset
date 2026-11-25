//! Procedural material table: material id -> pattern, colours, scale and
//! animation speed, loaded from RON and mirrored in a GPU uniform buffer.

use std::path::Path;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Procedural pattern of a material, evaluated in the fragment shader.
///
/// The discriminants are the ids the shader switches on
/// (`faset-shaders/src/materials.wgsl`; ids 14 and up are the pixel-art
/// patterns of `pixel.wgsl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pattern {
    /// Colour A only.
    Flat = 0,
    /// Colour A at height 0 to colour B at height `scale`.
    Gradient = 1,
    /// Slight variation between A and B (sand, rock, grass).
    Noise = 2,
    Checker = 3,
    /// Stripes half a cell wide.
    Stripes = 4,
    /// Lines (colour B) on a one-cell grid.
    Grid = 5,
    /// Bricks one by half a cell, mortar in colour B.
    Brick = 6,
    /// Boards a quarter cell wide, two cells long, seams in colour B.
    Planks = 7,
    /// One-cell tiles, grout in colour B.
    Tiles = 8,
    /// Wind ripples (dunes, snow).
    Ripples = 9,
    /// Animated waves.
    Water = 10,
    /// Animated glowing cracks (colour B).
    Lava = 11,
    /// Colour A, pulsing with `speed`.
    Emissive = 12,
    /// Scrolling scanlines between A and B with flicker.
    Hologram = 13,
    /// Pixel bricks, mortar in colour B.
    PixelBrick = 14,
    /// Pixel boards, seams dark (colour B).
    PixelWood = 15,
    /// Pixel grass: blades in colour B.
    PixelGrass = 16,
    /// Pixel sand with ripples; pebbles in colour B.
    PixelSand = 17,
    /// Pixel cobblestones, joints in colour B.
    PixelStone = 18,
    /// Pixel concrete panels.
    PixelConcrete = 19,
    /// Pixel metal plates with rivets (colour B).
    PixelMetal = 20,
    /// Animated pixel water, sparkles in colour B.
    PixelWater = 21,
    /// Pixel tiles in a checker of A and B.
    PixelTile = 22,
    /// Pixel earth with pebbles (colour B).
    PixelDirt = 23,
    /// Pixel snow with cool flecks (colour B).
    PixelSnow = 24,
    /// Pixel crate: frame and brace A, boards B.
    PixelCrate = 25,
    /// Pixel hazard stripes, A and B.
    PixelHazard = 26,
    /// Pixel foliage clumps, gaps in colour B.
    PixelLeaves = 27,
    /// Terrain ground (lot 6.10): two pixel materials blended by slope.
    /// `color_a` holds their ids (x: flat ground, y: steep slopes, both of a
    /// pixel pattern), `color_b` the slope band in `1 - n_y` (x: start,
    /// y: end); inside the band a per-texel dither picks the layer. Not in
    /// [`Pattern::ALL`], which lists the patterns with a swatch of their own.
    Terrain = 28,
    /// Layered terrain ground (lot 8.1): six pixel materials, all ids of a
    /// pixel pattern. `color_a` holds the palette ground, dirt and sand,
    /// `color_b` snow (or mud), painted rock and cliff rock. The vertex colour
    /// is not a tint but the weights of dirt (r), sand (g), snow (b) and
    /// painted rock (normal w); the ground takes the rest. Cliff rock comes
    /// from the slope (`1 - n_y` from 0.12 to 0.30) and is mapped on the
    /// dominant axis plane, so cliffs do not stretch. Up close a detail
    /// normal roughens the light. A per-texel dither picks one layer by
    /// weight; texels below a pixel blend the layers' means. Not in
    /// [`Pattern::ALL`].
    TerrainLayers = 29,
}

impl Pattern {
    /// Every pattern, in id order.
    pub const ALL: [Self; 28] = [
        Self::Flat,
        Self::Gradient,
        Self::Noise,
        Self::Checker,
        Self::Stripes,
        Self::Grid,
        Self::Brick,
        Self::Planks,
        Self::Tiles,
        Self::Ripples,
        Self::Water,
        Self::Lava,
        Self::Emissive,
        Self::Hologram,
        Self::PixelBrick,
        Self::PixelWood,
        Self::PixelGrass,
        Self::PixelSand,
        Self::PixelStone,
        Self::PixelConcrete,
        Self::PixelMetal,
        Self::PixelWater,
        Self::PixelTile,
        Self::PixelDirt,
        Self::PixelSnow,
        Self::PixelCrate,
        Self::PixelHazard,
        Self::PixelLeaves,
    ];
}

/// One material: what the fragment shader paints for a material id.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub id: u8,
    pub pattern: Pattern,
    /// Linear RGB; may exceed 1 for emissive surfaces.
    pub color_a: [f32; 3],
    pub color_b: [f32; 3],
    /// Metres per pattern cell (> 0).
    pub scale: f32,
    /// Animation speed in cycles per second (animated patterns only).
    #[serde(default)]
    pub speed: f32,
}

/// Material table errors.
#[derive(Debug, thiserror::Error)]
pub enum MaterialError {
    #[error("cannot read material table: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid material table: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("material id {0} is defined twice")]
    DuplicateId(u8),
    #[error("material {0}: scale must be finite and > 0, colours and speed finite")]
    InvalidValue(u8),
    /// A `Terrain` or `TerrainLayers` material whose layers are not ids of
    /// pixel materials.
    #[error("terrain material {0}: its layers must be ids of pixel-pattern materials")]
    InvalidTerrainLayer(u8),
}

/// Materials by id (0..=255). Ids without an entry render as flat white,
/// which leaves the vertex colour unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialTable {
    entries: Vec<Option<Material>>,
}

/// The built-in table (`render/data/materials.ron`).
const BUILTIN: &str = include_str!("../data/materials.ron");

impl MaterialTable {
    /// Parses a RON list of materials.
    pub fn from_ron(text: &str) -> Result<Self, MaterialError> {
        let list: Vec<Material> = ron::from_str(text)?;
        Self::from_materials(list)
    }

    /// Reads and parses a RON file.
    pub fn load(path: &Path) -> Result<Self, MaterialError> {
        Self::from_ron(&std::fs::read_to_string(path)?)
    }

    /// Builds a table, rejecting duplicate ids and invalid values.
    pub fn from_materials(list: impl IntoIterator<Item = Material>) -> Result<Self, MaterialError> {
        let mut entries = vec![None; 256];
        for m in list {
            let finite =
                m.color_a.iter().chain(&m.color_b).all(|c| c.is_finite()) && m.speed.is_finite();
            if !finite || !m.scale.is_finite() || m.scale <= 0.0 {
                return Err(MaterialError::InvalidValue(m.id));
            }
            let slot = &mut entries[usize::from(m.id)];
            if slot.is_some() {
                return Err(MaterialError::DuplicateId(m.id));
            }
            *slot = Some(m);
        }
        let table = Self { entries };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pixel = |v: f32| {
            v.fract() == 0.0
                && (0.0..=255.0).contains(&v)
                && table.get(v as u8).is_some_and(|l| {
                    (Pattern::PixelBrick as u32..=Pattern::PixelLeaves as u32)
                        .contains(&(l.pattern as u32))
                })
        };
        for m in table.iter() {
            let layers: &[f32] = match m.pattern {
                Pattern::Terrain => &m.color_a[..2],
                Pattern::TerrainLayers => &[
                    m.color_a[0],
                    m.color_a[1],
                    m.color_a[2],
                    m.color_b[0],
                    m.color_b[1],
                    m.color_b[2],
                ],
                _ => &[],
            };
            if !layers.iter().all(|&v| pixel(v)) {
                return Err(MaterialError::InvalidTerrainLayer(m.id));
            }
        }
        Ok(table)
    }

    /// The built-in table: ids 0..=27 show one pattern each, 28..=49 are
    /// colour variants of the pixel patterns.
    pub fn builtin() -> Self {
        Self::from_ron(BUILTIN).expect("the built-in material table is valid")
    }

    pub fn get(&self, id: u8) -> Option<&Material> {
        self.entries[usize::from(id)].as_ref()
    }

    /// Defined materials in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Material> {
        self.entries.iter().flatten()
    }

    /// The GPU mirror: 256 entries, missing ids as flat white.
    pub(crate) fn to_gpu(&self) -> Vec<GpuMaterial> {
        self.entries
            .iter()
            .map(|e| match e {
                Some(m) => GpuMaterial {
                    color_a: [m.color_a[0], m.color_a[1], m.color_a[2], 0.0],
                    color_b: [m.color_b[0], m.color_b[1], m.color_b[2], 0.0],
                    pattern: m.pattern as u32,
                    scale: m.scale,
                    speed: m.speed,
                    _pad: 0.0,
                },
                None => GpuMaterial::WHITE,
            })
            .collect()
    }
}

impl Default for MaterialTable {
    fn default() -> Self {
        Self::builtin()
    }
}

/// `Material` in the shader's uniform layout (48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuMaterial {
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    pub pattern: u32,
    pub scale: f32,
    pub speed: f32,
    pub _pad: f32,
}

impl GpuMaterial {
    const WHITE: Self = Self {
        color_a: [1.0; 4],
        color_b: [1.0; 4],
        pattern: Pattern::Flat as u32,
        scale: 1.0,
        speed: 0.0,
        _pad: 0.0,
    };
}

/// Bytes of the GPU material table.
pub(crate) const MATERIAL_TABLE_SIZE: u64 = 256 * std::mem::size_of::<GpuMaterial>() as u64;

const _: () = assert!(std::mem::size_of::<GpuMaterial>() == 48);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_parses_with_one_material_per_pattern() {
        let table = MaterialTable::builtin();
        for (id, pattern) in Pattern::ALL.iter().enumerate() {
            let m = table.get(id as u8).expect("ids 0..=27 exist");
            assert_eq!(m.pattern, *pattern);
            assert_eq!(m.id, id as u8);
            assert_eq!(*pattern as usize, id);
        }
        assert!(table.iter().count() >= Pattern::ALL.len());
        assert_eq!(
            table.get(0).unwrap().color_a,
            [1.0; 3],
            "id 0 keeps vertex colours"
        );
    }

    #[test]
    fn missing_id_is_none_and_white_on_gpu() {
        let table = MaterialTable::from_ron(
            "[(id: 7, pattern: Checker, color_a: (1.0, 0.0, 0.0), color_b: (0.0, 0.0, 1.0), scale: 2.0, speed: 0.5)]",
        )
        .unwrap();
        let m = table.get(7).unwrap();
        assert_eq!(m.pattern, Pattern::Checker);
        assert_eq!((m.scale, m.speed), (2.0, 0.5));
        assert!(table.get(0).is_none());
        assert!(table.get(255).is_none());
        let gpu = table.to_gpu();
        assert_eq!(gpu.len(), 256);
        assert_eq!(gpu[0], GpuMaterial::WHITE);
        assert_eq!(gpu[7].pattern, 3);
        assert_eq!(gpu[7].color_b, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&gpu).len() as u64,
            MATERIAL_TABLE_SIZE
        );
    }

    #[test]
    fn terrain_materials_blend_two_pixel_layers() {
        let table = MaterialTable::builtin();
        let terrain: Vec<_> = table
            .iter()
            .filter(|m| m.pattern == Pattern::Terrain)
            .collect();
        assert_eq!(terrain.len(), 5);
        for m in terrain {
            assert!((54..=58).contains(&m.id));
            assert!(m.color_b[0] < m.color_b[1], "slope band of {}", m.id);
        }
        let bad = |layer: f32| {
            MaterialTable::from_ron(&format!(
                "[(id: 3, pattern: Checker, color_a: (1.0, 1.0, 1.0), color_b: (1.0, 1.0, 1.0), scale: 1.0), \
                  (id: 16, pattern: PixelGrass, color_a: (1.0, 1.0, 1.0), color_b: (1.0, 1.0, 1.0), scale: 1.0), \
                  (id: 54, pattern: Terrain, color_a: (16.0, {layer:.1}, 0.0), color_b: (0.1, 0.2, 0.0), scale: 1.0)]"
            ))
        };
        assert!(bad(16.0).is_ok());
        for layer in [3.0, 7.0, 54.0, 16.5, 300.0] {
            assert!(
                matches!(bad(layer), Err(MaterialError::InvalidTerrainLayer(54))),
                "{layer}"
            );
        }
    }

    #[test]
    fn layered_terrain_materials_hold_six_pixel_layers() {
        let table = MaterialTable::builtin();
        let sets: Vec<_> = table
            .iter()
            .filter(|m| m.pattern == Pattern::TerrainLayers)
            .collect();
        assert_eq!(sets.len(), 4);
        for m in sets {
            assert!((59..=62).contains(&m.id));
        }
        let with = |rock: f32| {
            MaterialTable::from_ron(&format!(
                "[(id: 16, pattern: PixelGrass, color_a: (1.0, 1.0, 1.0), color_b: (1.0, 1.0, 1.0), scale: 1.0), \
                  (id: 59, pattern: TerrainLayers, color_a: (16.0, 16.0, 16.0), color_b: (16.0, 16.0, {rock:.1}), scale: 1.0)]"
            ))
        };
        assert!(with(16.0).is_ok());
        for rock in [0.0, 59.0, 16.5] {
            assert!(matches!(
                with(rock),
                Err(MaterialError::InvalidTerrainLayer(59))
            ));
        }
    }

    #[test]
    fn invalid_tables_are_rejected() {
        let one = |id: u8, scale: f32| Material {
            id,
            pattern: Pattern::Flat,
            color_a: [1.0; 3],
            color_b: [1.0; 3],
            scale,
            speed: 0.0,
        };
        assert!(matches!(
            MaterialTable::from_materials([one(3, 1.0), one(3, 2.0)]),
            Err(MaterialError::DuplicateId(3))
        ));
        assert!(matches!(
            MaterialTable::from_materials([one(4, 0.0)]),
            Err(MaterialError::InvalidValue(4))
        ));
        assert!(matches!(
            MaterialTable::from_ron("[(id: 1, pattern: Marble, color_a: (1.0, 1.0, 1.0), color_b: (1.0, 1.0, 1.0), scale: 1.0)]"),
            Err(MaterialError::Parse(_))
        ));
        assert!(matches!(
            MaterialTable::from_ron("[(id: 300, pattern: Flat, color_a: (1.0, 1.0, 1.0), color_b: (1.0, 1.0, 1.0), scale: 1.0)]"),
            Err(MaterialError::Parse(_))
        ));
    }
}
