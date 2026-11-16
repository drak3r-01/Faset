//! Procedural material table: material id -> pattern, colours, scale and
//! animation speed, loaded from RON and mirrored in a GPU uniform buffer.

use std::path::Path;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Procedural pattern of a material, evaluated in the fragment shader.
///
/// The discriminants are the ids the shader switches on
/// (`faset-shaders/src/materials.wgsl`).
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
}

impl Pattern {
    /// Every pattern, in id order.
    pub const ALL: [Self; 14] = [
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
        Ok(Self { entries })
    }

    /// The built-in table: ids 0..=13 show one pattern each.
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
            let m = table.get(id as u8).expect("ids 0..=13 exist");
            assert_eq!(m.pattern, *pattern);
            assert_eq!(m.id, id as u8);
            assert_eq!(*pattern as usize, id);
        }
        assert_eq!(table.iter().count(), Pattern::ALL.len());
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
