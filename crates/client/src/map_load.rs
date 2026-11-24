//! Map loading: file bytes to collision world and render meshes.
//!
//! Implements steps 1 to 3 of the load pipeline: read, migrate and expand
//! the file, build the meshes with baked ambient occlusion, build the
//! collision world. [`load_map`] needs no GPU; only [`LoadedMap::upload`]
//! touches the renderer, so a headless build can skip it.

use faset_mapfmt::geometry::{bake_occlusion, build_meshes, MeshChunk};
use faset_mapfmt::vegetation::{scatter, PlacedPlant};
use faset_mapfmt::{MapError, MapFile};
use faset_render::glam::Mat4;
use faset_render::{Instance, RenderError, Renderer, Vertex};
use faset_sim::MapCollision;

/// Why a map could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum MapLoadError {
    #[error("map file rejected: {0}")]
    Map(#[from] MapError),
}

/// A map ready to play: expanded file, collision world, mesh chunks and
/// plants.
pub struct LoadedMap {
    /// The expanded map (prefabs and symmetry applied).
    pub map: MapFile,
    pub collision: MapCollision,
    /// One chunk per 16 m block, with ambient occlusion in the colours.
    pub chunks: Vec<MeshChunk>,
    /// Every plant of the map (lot 6.11), the same the bots' sight uses.
    pub plants: Vec<PlacedPlant>,
}

/// Parses, expands and builds a map file.
///
/// # Errors
/// [`MapLoadError::Map`] if the file is not a valid map.
pub fn load_map(bytes: &[u8]) -> Result<LoadedMap, MapLoadError> {
    let map = faset_mapfmt::expand(&faset_mapfmt::parse(bytes)?);
    let collision = MapCollision::build(&map);
    let mut chunks = build_meshes(&map);
    bake_occlusion(&mut chunks, |origin, dir, max| {
        collision.ray_cast(origin, dir, max).is_some()
    });
    let plants = scatter(&map);
    tracing::info!(
        name = %map.meta.name,
        volumes = collision.volume_count(),
        chunks = chunks.len(),
        props = map.props.len(),
        plants = plants.len(),
        "map built"
    );
    Ok(LoadedMap {
        map,
        collision,
        chunks,
        plants,
    })
}

impl LoadedMap {
    /// Uploads every chunk and prop to the renderer and returns their static
    /// instances (chunks first, identity transform); hands the plants to the
    /// renderer's foliage layer (drawn when
    /// [`RenderWorld::foliage`](faset_render::RenderWorld::foliage) is set).
    ///
    /// # Errors
    /// Any [`RenderError`] of the mesh upload (for example a full arena).
    pub fn upload(&self, renderer: &mut Renderer) -> Result<Vec<Instance>, RenderError> {
        let mut instances = self.upload_blocks(renderer)?;
        instances.extend(crate::scenery::prop_instances(renderer, &self.map)?);
        let theme = crate::scenery::plant_theme(&self.map.theme.palette);
        let (kinds, plants) = crate::scenery::foliage_layer(renderer, &self.plants, theme)?;
        renderer.set_foliage(kinds, &plants)?;
        Ok(instances)
    }

    /// Uploads every chunk to the renderer and returns one static instance
    /// each (identity transform).
    ///
    /// # Errors
    /// Any [`RenderError`] of the mesh upload (for example a full arena).
    pub fn upload_blocks(&self, renderer: &mut Renderer) -> Result<Vec<Instance>, RenderError> {
        let mut instances = Vec::with_capacity(self.chunks.len());
        for chunk in &self.chunks {
            let vertices: Vec<Vertex> = chunk
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color_material: v.color_material,
                })
                .collect();
            let mesh = renderer.upload_mesh(&vertices, &chunk.indices)?;
            instances.push(Instance::new(mesh, 0, Mat4::IDENTITY));
        }
        Ok(instances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../assets/testdata/example.ron");

    #[test]
    fn example_map_loads() {
        let m = load_map(EXAMPLE.as_bytes()).unwrap();
        assert!(!m.chunks.is_empty());
        assert!(m.collision.volume_count() > 0);
        for c in &m.chunks {
            assert!(!c.indices.is_empty());
        }
    }

    #[test]
    fn bad_file_is_rejected() {
        assert!(load_map(b"not a map").is_err());
    }
}
