//! Shelter map: a height map seen from above, built once at map load, that
//! masks weather particles under a roof.
//!
//! Each cell holds the height of the highest solid surface above it. A
//! particle whose height is below the cell's height is under a roof (or
//! inside a building) and is not drawn. The map is uploaded to a storage
//! buffer the particle vertex shader reads (`weather.wgsl`).

use ash::vk;
use faset_core::glam::{Vec2, Vec3};
use gpu_allocator::MemoryLocation;

use crate::context::Context;
use crate::gpu::GpuBuffer;
use crate::RenderError;

/// Height of a cell that has nothing above it, in metres.
pub const NO_ROOF: f32 = -1.0e4;

/// Largest map side in cells (a 4096 x 4096 map is 64 MiB).
pub const MAX_SHELTER_SIDE: u32 = 4096;

/// Why a [`ShelterMap`] cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShelterError {
    #[error("shelter map size {width} x {height} must be 1 to {MAX_SHELTER_SIDE} cells per side")]
    Size { width: u32, height: u32 },
    #[error("{got} heights given for a {width} x {height} shelter map")]
    HeightCount { width: u32, height: u32, got: usize },
    #[error("shelter map cell size must be finite and positive")]
    CellSize,
}

/// A height map of the roofs of a map, seen from above.
#[derive(Debug, Clone, PartialEq)]
pub struct ShelterMap {
    width: u32,
    height: u32,
    /// World X and Z of the minimum corner of cell (0, 0).
    origin: Vec2,
    cell_size: f32,
    /// Row-major, row 0 at the minimum Z.
    heights: Vec<f32>,
}

impl ShelterMap {
    /// A map with no roofs: nothing is sheltered.
    pub fn empty() -> Self {
        Self {
            width: 1,
            height: 1,
            origin: Vec2::ZERO,
            cell_size: 1.0,
            heights: vec![NO_ROOF],
        }
    }

    /// Builds a map from per-cell heights of the highest solid surface, in
    /// metres (use [`NO_ROOF`] for open sky). `heights` is row-major with
    /// `width` cells per row, row 0 at the minimum Z; `origin` is the world X
    /// and Z of the minimum corner of cell (0, 0). Non-finite heights count
    /// as [`NO_ROOF`].
    pub fn from_heights(
        width: u32,
        height: u32,
        origin: Vec2,
        cell_size: f32,
        heights: &[f32],
    ) -> Result<Self, ShelterError> {
        if !(1..=MAX_SHELTER_SIDE).contains(&width) || !(1..=MAX_SHELTER_SIDE).contains(&height) {
            return Err(ShelterError::Size { width, height });
        }
        if heights.len() != width as usize * height as usize {
            return Err(ShelterError::HeightCount {
                width,
                height,
                got: heights.len(),
            });
        }
        if !(cell_size.is_finite() && cell_size > 0.0) || !origin.is_finite() {
            return Err(ShelterError::CellSize);
        }
        Ok(Self {
            width,
            height,
            origin,
            cell_size,
            heights: heights
                .iter()
                .map(|&h| if h.is_finite() { h } else { NO_ROOF })
                .collect(),
        })
    }

    /// Rasterises axis-aligned boxes `(min, max)` seen from above: every
    /// cell a box covers takes the highest top among the boxes over it.
    pub fn from_boxes(
        origin: Vec2,
        width: u32,
        height: u32,
        cell_size: f32,
        boxes: &[(Vec3, Vec3)],
    ) -> Result<Self, ShelterError> {
        let mut heights = vec![NO_ROOF; width as usize * height as usize];
        if (1..=MAX_SHELTER_SIDE).contains(&width)
            && (1..=MAX_SHELTER_SIDE).contains(&height)
            && cell_size.is_finite()
            && cell_size > 0.0
        {
            for (min, max) in boxes {
                let cell = |v: f32, o: f32, n: u32| {
                    (((v - o) / cell_size).floor().clamp(-1.0, n as f32)) as i64
                };
                let x0 = cell(min.x, origin.x, width).max(0);
                let x1 = cell(max.x - 1e-4, origin.x, width).min(i64::from(width) - 1);
                let z0 = cell(min.z, origin.y, height).max(0);
                let z1 = cell(max.z - 1e-4, origin.y, height).min(i64::from(height) - 1);
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        let slot = &mut heights[(z as usize) * width as usize + x as usize];
                        *slot = slot.max(max.y);
                    }
                }
            }
        }
        Self::from_heights(width, height, origin, cell_size, &heights)
    }

    /// Cells per row.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Rows.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Height of the highest surface above world position (`x`, `z`), or
    /// `None` outside the map.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let cx = ((x - self.origin.x) / self.cell_size).floor();
        let cz = ((z - self.origin.y) / self.cell_size).floor();
        if !(0.0..self.width as f32).contains(&cx) || !(0.0..self.height as f32).contains(&cz) {
            return None;
        }
        Some(self.heights[cz as usize * self.width as usize + cx as usize])
    }

    /// True when a point is under a roof: the map's height above it is
    /// greater than its own. Points outside the map are not sheltered. Same
    /// test as the particle vertex shader.
    pub fn is_sheltered(&self, p: Vec3) -> bool {
        self.height_at(p.x, p.z).is_some_and(|h| h > p.y)
    }

    pub(crate) fn info(&self) -> ShelterInfo {
        ShelterInfo {
            origin: self.origin.to_array(),
            cell_size: self.cell_size,
            width: self.width,
            height: self.height,
            enabled: self.heights.iter().any(|&h| h > NO_ROOF),
        }
    }
}

/// What the lighting block says about the loaded shelter map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShelterInfo {
    pub origin: [f32; 2],
    pub cell_size: f32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
}

/// The map's heights in a host-visible storage buffer.
pub(crate) struct ShelterBuffer {
    pub buffer: GpuBuffer,
    pub info: ShelterInfo,
}

impl ShelterBuffer {
    pub(crate) fn new(ctx: &mut Context, map: &ShelterMap) -> Result<Self, RenderError> {
        let bytes: &[u8] = bytemuck::cast_slice(&map.heights);
        let mut buffer = GpuBuffer::new(
            ctx,
            "shelter-map",
            bytes.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::CpuToGpu,
        )?;
        buffer.mapped()[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            buffer,
            info: map.info(),
        })
    }

    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        self.buffer.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_heights_validates_its_input() {
        let ok =
            |w, h, n: usize, cell| ShelterMap::from_heights(w, h, Vec2::ZERO, cell, &vec![1.0; n]);
        assert!(ok(2, 3, 6, 1.0).is_ok());
        assert_eq!(
            ok(2, 3, 5, 1.0),
            Err(ShelterError::HeightCount {
                width: 2,
                height: 3,
                got: 5
            })
        );
        assert!(matches!(ok(0, 3, 0, 1.0), Err(ShelterError::Size { .. })));
        assert!(matches!(
            ok(MAX_SHELTER_SIDE + 1, 1, 0, 1.0),
            Err(ShelterError::Size { .. })
        ));
        assert_eq!(ok(1, 1, 1, 0.0), Err(ShelterError::CellSize));
        assert_eq!(ok(1, 1, 1, f32::NAN), Err(ShelterError::CellSize));
    }

    #[test]
    fn lookup_follows_the_origin_and_the_cell_size() {
        // 2 x 2 cells of 2 m starting at (-2, 10): heights by (x, z).
        let map = ShelterMap::from_heights(2, 2, Vec2::new(-2.0, 10.0), 2.0, &[1.0, 2.0, 3.0, 4.0])
            .unwrap();
        assert_eq!(map.height_at(-1.0, 11.0), Some(1.0));
        assert_eq!(map.height_at(1.0, 11.0), Some(2.0));
        assert_eq!(map.height_at(-1.0, 13.0), Some(3.0));
        assert_eq!(map.height_at(1.9, 13.9), Some(4.0));
        assert_eq!(map.height_at(2.0, 11.0), None);
        assert_eq!(map.height_at(-2.1, 11.0), None);
        assert_eq!(map.height_at(0.0, 9.9), None);
        assert!(map.is_sheltered(Vec3::new(1.0, 1.5, 11.0)));
        assert!(!map.is_sheltered(Vec3::new(1.0, 2.5, 11.0)));
        assert!(!map.is_sheltered(Vec3::new(50.0, 0.0, 50.0)), "outside");
    }

    #[test]
    fn boxes_are_rasterised_to_their_tops() {
        let map = ShelterMap::from_boxes(
            Vec2::ZERO,
            8,
            8,
            1.0,
            &[
                (Vec3::new(1.0, 0.0, 1.0), Vec3::new(4.0, 3.0, 3.0)),
                (Vec3::new(2.0, 0.0, 2.0), Vec3::new(3.0, 5.0, 3.0)),
                // Partly off the map.
                (Vec3::new(-5.0, 0.0, 6.0), Vec3::new(1.5, 2.0, 20.0)),
            ],
        )
        .unwrap();
        assert_eq!(map.height_at(1.5, 1.5), Some(3.0));
        assert_eq!(map.height_at(3.5, 2.5), Some(3.0));
        assert_eq!(map.height_at(2.5, 2.5), Some(5.0), "taller box wins");
        assert_eq!(map.height_at(4.5, 1.5), Some(NO_ROOF));
        assert_eq!(map.height_at(0.5, 7.5), Some(2.0));
        assert_eq!(map.height_at(1.5, 7.5), Some(2.0));
        assert_eq!(map.height_at(2.5, 7.5), Some(NO_ROOF));
        assert!(!ShelterMap::empty().is_sheltered(Vec3::ZERO));
        assert!(!ShelterMap::empty().info().enabled);
        assert!(map.info().enabled);
    }
}
