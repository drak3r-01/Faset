//! The world terrain (lot 8.1): the 1 to 2 km ground of a map with a
//! [`World`] block, drawn as a chunked quadtree with levels of detail and
//! streamed into a fixed pool of renderer slots.
//!
//! [`WorldTerrain`] samples the world surface ([`World::height_at`]: the
//! playable terrain on its own grid, the far field around it) once at load on
//! a fine grid aligned with the playable terrain, so the finest level is the
//! collision surface of lot 6.10 triangle for triangle. Every quadtree node
//! is a chunk of [`CHUNK_CELLS`] x [`CHUNK_CELLS`] cells; a node of level `k`
//! takes every `2^k`-th fine sample. Each node knows its geometric error (the
//! largest height difference between its mesh and the fine surface, never
//! smaller than its children's).
//!
//! [`TerrainStream`] picks, every frame, the coarsest nodes whose error seen
//! from the eye stays under [`PIXEL_ERROR`] pixels (so switching level moves
//! the ground by less than about a pixel: no visible popping) and draws the
//! resident ones; a node whose children are not all resident yet is drawn
//! instead of them, so the ground never has holes. Missing nodes are built
//! and written to free or least recently used slots, the most visibly wrong
//! first and those in the view before the others, at most
//! [`MAX_BUILDS_PER_FRAME`] per frame and within the renderer's upload budget.
//! The choice depends on distance only, so turning the camera loads nothing;
//! the renderer culls the chunks against the view. Vertical skirts along every
//! chunk edge, shaded like the edge, hide the cracks between levels.
//!
//! Vertices carry per-vertex smooth normals (central differences on the fine
//! grid, the same at every level) and the weights of the layered terrain
//! material (render pattern `TerrainLayers`): dirt, sand, snow and painted
//! rock from the paint of the map, sand on the shore below the sea level plus
//! 1.8 m, snow above the world's snow line.
//!
//! Decisions of lot 8.1:
//! - 32 cells per chunk; 160 slots of 1 221 vertices and 7 680 indices (about
//!   55 KB each, 8.8 MB in all); 4 chunk builds per frame at most;
//! - the screen-space error budget is 1 pixel at [`PROJECTION_SCALE`] (a
//!   1080-pixel-high view with a 75 degree vertical field of view);
//! - skirts hang `2 * e + 0.5` m below the edge, `e` the error of the node's
//!   grandparent (its own error when it has none), which covers neighbours up
//!   to two levels coarser.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use faset_mapfmt::geometry::terrain_layer_material;
use faset_mapfmt::world::BLEND_M;
use faset_mapfmt::{MapFile, Terrain, World};
use faset_render::glam::{Mat4, Vec3, Vec4};
use faset_render::{Instance, MeshId, RenderError, Renderer, Vertex};

/// Cells along each side of a chunk.
pub const CHUNK_CELLS: usize = 32;
/// Streamed slots, the most chunks resident at once.
pub const SLOT_COUNT: u32 = 160;
/// Vertices of a full chunk: the surface and the skirt bottoms.
pub const SLOT_VERTICES: u32 =
    ((CHUNK_CELLS + 1) * (CHUNK_CELLS + 1) + 4 * (CHUNK_CELLS + 1)) as u32;
/// Indices of a full chunk: the surface and both faces of the skirts.
pub const SLOT_INDICES: u32 = (CHUNK_CELLS * CHUNK_CELLS * 6 + 4 * CHUNK_CELLS * 12) as u32;
/// Bytes of a full chunk.
pub const CHUNK_BYTES: usize = SLOT_VERTICES as usize * 20 + SLOT_INDICES as usize * 4;
/// Most chunks built per frame.
pub const MAX_BUILDS_PER_FRAME: usize = 4;
/// Screen-space error allowed for a level, in pixels.
pub const PIXEL_ERROR: f32 = 1.0;
/// Pixels per unit of `error / distance`: half a 1080-pixel view height over
/// `tan(37.5 degrees)`.
pub const PROJECTION_SCALE: f32 = 704.0;
/// Fine grid spacing of a world without a playable terrain, metres.
pub const DEFAULT_CELL_M: f32 = 2.0;
/// Most fine samples (the fine grid is coarsened above it).
pub const MAX_FINE_SAMPLES: usize = 2049 * 2049;
/// Width of the shore band above the sea level, metres.
const SHORE_M: f32 = 1.8;

/// A quadtree node: its level (0 is the finest) and its chunk coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeKey {
    pub level: u8,
    pub x: u16,
    pub z: u16,
}

/// The ground of a world map, sampled once on the fine grid.
#[derive(Debug, Clone)]
pub struct WorldTerrain {
    /// World position of fine sample `(0, 0)`, `(x, z)` metres.
    origin: [f32; 2],
    cell: f32,
    nx: usize,
    nz: usize,
    heights: Vec<f32>,
    /// dirt, sand, snow, painted rock (0..255).
    splat: Vec<[u8; 4]>,
    /// How much of the far field a sample is (0 on the playable terrain,
    /// 255 past the blend band): its normal is smoothed over a far cell.
    far: Vec<u8>,
    /// Fine cells per far cell: the stencil of the smoothed normals.
    far_cells: usize,
    material: u8,
    /// Per level: nodes along x and z, then error and height range per node.
    levels: Vec<Level>,
}

#[derive(Debug, Clone)]
struct Level {
    nodes_x: usize,
    nodes_z: usize,
    error: Vec<f32>,
    range: Vec<(f32, f32)>,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl WorldTerrain {
    /// Samples the world of `map`; `None` when it has no [`World`] block.
    #[must_use]
    pub fn build(map: &MapFile) -> Option<Self> {
        let world = map.world.as_ref()?;
        let sea = map.surroundings.as_ref().and_then(|s| s.sea_level);
        Some(Self::sample(
            world,
            map.terrain.as_ref(),
            terrain_layer_material(&map.theme.palette),
            sea,
        ))
    }

    /// Samples `world` (with its playable `terrain`) on the fine grid.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn sample(
        world: &World,
        terrain: Option<&Terrain>,
        material: u8,
        sea: Option<f32>,
    ) -> Self {
        let [x0, z0, x1, z1] = world.extent;
        let mut cell = terrain.map_or(DEFAULT_CELL_M, Terrain::cell_m);
        // The grid goes through the terrain's samples and covers the extent.
        let anchor = terrain.map_or([x0, z0], |t| t.extent_xz().0);
        let (origin, nx, nz) = loop {
            let ox = anchor[0] - ((anchor[0] - x0) / cell).ceil() * cell;
            let oz = anchor[1] - ((anchor[1] - z0) / cell).ceil() * cell;
            let nx = ((x1 - ox) / cell).ceil() as usize + 1;
            let nz = ((z1 - oz) / cell).ceil() as usize + 1;
            if nx * nz <= MAX_FINE_SAMPLES {
                break ([ox, oz], nx.max(2), nz.max(2));
            }
            cell *= 2.0;
        };
        let mut heights = Vec::with_capacity(nx * nz);
        let mut splat = Vec::with_capacity(nx * nz);
        let mut far = Vec::with_capacity(nx * nz);
        let rect = terrain.map(Terrain::extent_xz);
        for j in 0..nz {
            for i in 0..nx {
                let x = origin[0] + i as f32 * cell;
                let z = origin[1] + j as f32 * cell;
                let h = world.height_at(terrain, x, z);
                far.push(rect.map_or(255, |(lo, hi)| {
                    let dx = (lo[0] - x).max(x - hi[0]).max(0.0);
                    let dz = (lo[1] - z).max(z - hi[1]).max(0.0);
                    unorm(smoothstep(0.0, BLEND_M, dx.hypot(dz)))
                }));
                // PAINTS: auto, grass, sand, rock, snow, dirt.
                let p = world.paint_at(terrain, x, z);
                let shore = sea.map_or(0.0, |s| smoothstep(s + SHORE_M, s + 0.4, h));
                let snow = world
                    .snow_line
                    .map_or(0.0, |line| smoothstep(line - 4.0, line + 4.0, h));
                let mut w = [p[5], p[2].max(shore), p[4].max(snow), p[3]];
                let sum: f32 = w.iter().sum();
                if sum > 1.0 {
                    w = w.map(|v| v / sum);
                }
                heights.push(h);
                splat.push(w.map(unorm));
            }
        }
        let mut terrain = Self {
            origin,
            cell,
            nx,
            nz,
            heights,
            splat,
            far,
            far_cells: ((f32::from(world.far.cell) / cell).round() as usize).max(1),
            material,
            levels: Vec::new(),
        };
        terrain.build_levels();
        terrain
    }

    /// Fine samples along x and z.
    #[must_use]
    pub fn dims(&self) -> (usize, usize) {
        (self.nx, self.nz)
    }

    /// Fine grid spacing, metres.
    #[must_use]
    pub fn cell_m(&self) -> f32 {
        self.cell
    }

    /// Number of levels; the last one is a single root node.
    #[must_use]
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// The root node.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn root(&self) -> NodeKey {
        NodeKey {
            level: (self.levels.len() - 1) as u8,
            x: 0,
            z: 0,
        }
    }

    fn h(&self, i: usize, j: usize) -> f32 {
        self.heights[j.min(self.nz - 1) * self.nx + i.min(self.nx - 1)]
    }

    /// World position of fine sample `(i, j)`.
    #[allow(clippy::cast_precision_loss)]
    fn pos(&self, i: usize, j: usize) -> Vec3 {
        Vec3::new(
            self.origin[0] + i as f32 * self.cell,
            self.h(i, j),
            self.origin[1] + j as f32 * self.cell,
        )
    }

    /// Smooth normal of fine sample `(i, j)`: central differences over `r`
    /// samples each way.
    #[allow(clippy::cast_precision_loss)]
    fn gradient_normal(&self, i: usize, j: usize, r: usize) -> Vec3 {
        let (i0, i1) = (i.saturating_sub(r), (i + r).min(self.nx - 1));
        let (j0, j1) = (j.saturating_sub(r), (j + r).min(self.nz - 1));
        let dx = (self.h(i1, j) - self.h(i0, j)) / ((i1 - i0).max(1) as f32 * self.cell);
        let dz = (self.h(i, j1) - self.h(i, j0)) / ((j1 - j0).max(1) as f32 * self.cell);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Smooth normal of fine sample `(i, j)`: central differences on the
    /// playable terrain; on the far field (linear between its samples) over
    /// a far cell each way, so the light varies smoothly across its facets.
    fn normal(&self, i: usize, j: usize) -> Vec3 {
        let near = self.gradient_normal(i, j, 1);
        let t = f32::from(self.far[j.min(self.nz - 1) * self.nx + i.min(self.nx - 1)]) / 255.0;
        if t <= 0.0 || self.far_cells <= 1 {
            return near;
        }
        let far = self.gradient_normal(i, j, self.far_cells);
        near.lerp(far, t).normalize()
    }

    /// Fine-sample step and first fine sample of a node.
    fn span(key: NodeKey) -> (usize, usize, usize) {
        let s = 1usize << key.level;
        (
            s,
            usize::from(key.x) * CHUNK_CELLS * s,
            usize::from(key.z) * CHUNK_CELLS * s,
        )
    }

    fn level(&self, key: NodeKey) -> &Level {
        &self.levels[usize::from(key.level)]
    }

    fn node_index(&self, key: NodeKey) -> usize {
        usize::from(key.z) * self.level(key).nodes_x + usize::from(key.x)
    }

    /// Whether `key` is a node of this terrain.
    #[must_use]
    pub fn exists(&self, key: NodeKey) -> bool {
        self.levels
            .get(usize::from(key.level))
            .is_some_and(|l| usize::from(key.x) < l.nodes_x && usize::from(key.z) < l.nodes_z)
    }

    /// Geometric error of a node, metres.
    #[must_use]
    pub fn error(&self, key: NodeKey) -> f32 {
        self.level(key).error[self.node_index(key)]
    }

    /// Depth of a node's skirts below its edges, metres.
    #[must_use]
    pub fn skirt_depth(&self, key: NodeKey) -> f32 {
        let top = self.levels.len() - 1;
        let up = (usize::from(key.level) + 2).min(top);
        let shift = up - usize::from(key.level);
        #[allow(clippy::cast_possible_truncation)]
        let ancestor = NodeKey {
            level: up as u8,
            x: key.x >> shift,
            z: key.z >> shift,
        };
        2.0 * self.error(ancestor).max(self.error(key)) + 0.5
    }

    /// Bounds of a node (skirts included), `(min, max)`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn bounds(&self, key: NodeKey) -> (Vec3, Vec3) {
        let (s, i0, j0) = Self::span(key);
        let (lo_y, hi_y) = self.level(key).range[self.node_index(key)];
        let i1 = (i0 + CHUNK_CELLS * s).min(self.nx - 1);
        let j1 = (j0 + CHUNK_CELLS * s).min(self.nz - 1);
        let (a, b) = (self.pos(i0, j0), self.pos(i1, j1));
        (
            Vec3::new(a.x, lo_y - self.skirt_depth(key), a.z),
            Vec3::new(b.x, hi_y, b.z),
        )
    }

    /// The existing children of a node.
    #[must_use]
    pub fn children(&self, key: NodeKey) -> Vec<NodeKey> {
        if key.level == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(4);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let c = NodeKey {
                level: key.level - 1,
                x: key.x * 2 + dx,
                z: key.z * 2 + dz,
            };
            if self.exists(c) {
                out.push(c);
            }
        }
        out
    }

    /// Height of a node's mesh at fine sample `(i, j)` inside it: its own
    /// triangles over its level's grid.
    #[allow(clippy::cast_precision_loss)]
    fn node_height(&self, key: NodeKey, i: usize, j: usize) -> f32 {
        let (s, i0, j0) = Self::span(key);
        let (ca, cb) = ((i - i0) / s, (j - j0) / s);
        let (ia, ib) = (
            (i0 + ca * s).min(self.nx - 1),
            (i0 + (ca + 1) * s).min(self.nx - 1),
        );
        let (ja, jb) = (
            (j0 + cb * s).min(self.nz - 1),
            (j0 + (cb + 1) * s).min(self.nz - 1),
        );
        let fu = if ib > ia {
            (i - ia) as f32 / (ib - ia) as f32
        } else {
            0.0
        };
        let fv = if jb > ja {
            (j - ja) as f32 / (jb - ja) as f32
        } else {
            0.0
        };
        let (h00, h01, h10, h11) = (
            self.h(ia, ja),
            self.h(ia, jb),
            self.h(ib, ja),
            self.h(ib, jb),
        );
        if fv >= fu {
            h00 + fv * (h01 - h00) + fu * (h11 - h01)
        } else {
            h00 + fu * (h10 - h00) + fv * (h11 - h10)
        }
    }

    /// Errors and height ranges of every node, finest level first.
    #[allow(clippy::cast_possible_truncation)]
    fn build_levels(&mut self) {
        let cells = (self.nx - 1).max(self.nz - 1);
        let mut count = 1;
        while CHUNK_CELLS << (count - 1) < cells {
            count += 1;
        }
        self.levels.clear();
        for k in 0..count {
            let size = CHUNK_CELLS << k;
            let (nodes_x, nodes_z) = ((self.nx - 1).div_ceil(size), (self.nz - 1).div_ceil(size));
            let (nodes_x, nodes_z) = (nodes_x.max(1), nodes_z.max(1));
            let mut level = Level {
                nodes_x,
                nodes_z,
                error: vec![0.0; nodes_x * nodes_z],
                range: vec![(f32::MAX, f32::MIN); nodes_x * nodes_z],
            };
            self.levels.push(level.clone());
            for nz in 0..nodes_z {
                for nx in 0..nodes_x {
                    let key = NodeKey {
                        level: k as u8,
                        x: nx as u16,
                        z: nz as u16,
                    };
                    let (_, i0, j0) = Self::span(key);
                    let i1 = (i0 + size).min(self.nx - 1);
                    let j1 = (j0 + size).min(self.nz - 1);
                    let (mut err, mut lo, mut hi) = (0.0f32, f32::MAX, f32::MIN);
                    for j in j0..=j1 {
                        for i in i0..=i1 {
                            let h = self.h(i, j);
                            lo = lo.min(h);
                            hi = hi.max(h);
                            if k > 0 {
                                err = err.max((self.node_height(key, i, j) - h).abs());
                            }
                        }
                    }
                    for c in self.children(key) {
                        err = err.max(self.error(c));
                    }
                    let n = nz * nodes_x + nx;
                    level.error[n] = err;
                    level.range[n] = (lo, hi);
                }
            }
            self.levels[k] = level;
        }
    }

    /// Writes the mesh of a node into `vertices` and `indices` (cleared
    /// first): the surface, then the skirts.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn build_chunk(&self, key: NodeKey, vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
        vertices.clear();
        indices.clear();
        let (s, i0, j0) = Self::span(key);
        let n = CHUNK_CELLS + 1;
        let fine =
            |a: usize, b: usize| ((i0 + a * s).min(self.nx - 1), (j0 + b * s).min(self.nz - 1));
        let vertex = |i: usize, j: usize, drop: f32| {
            let mut v = Vertex::new(
                self.pos(i, j) - Vec3::Y * drop,
                self.normal(i, j),
                [0; 3],
                self.material,
            );
            let w = self.splat[j * self.nx + i];
            v.color_material = [w[0], w[1], w[2], self.material];
            v.normal[3] = ((u16::from(w[3]) * 127 + 127) / 255) as i8;
            v
        };
        for b in 0..n {
            for a in 0..n {
                let (i, j) = fine(a, b);
                vertices.push(vertex(i, j, 0.0));
            }
        }
        // Cells inside the grid only (a node may overhang the far edge).
        let live_a = |a: usize| i0 + a * s < self.nx - 1;
        let live_b = |b: usize| j0 + b * s < self.nz - 1;
        let at = |a: usize, b: usize| (b * n + a) as u32;
        for b in (0..CHUNK_CELLS).filter(|&b| live_b(b)) {
            for a in (0..CHUNK_CELLS).filter(|&a| live_a(a)) {
                let (v00, v01, v10, v11) = (at(a, b), at(a, b + 1), at(a + 1, b), at(a + 1, b + 1));
                indices.extend_from_slice(&[v00, v01, v11, v00, v11, v10]);
            }
        }
        // Skirts: each edge's vertices again, lowered, both faces.
        let depth = self.skirt_depth(key);
        let last_a = (0..CHUNK_CELLS).rfind(|&a| live_a(a)).map_or(0, |a| a + 1);
        let last_b = (0..CHUNK_CELLS).rfind(|&b| live_b(b)).map_or(0, |b| b + 1);
        let edges: [Vec<(usize, usize)>; 4] = [
            (0..=last_a).map(|a| (a, 0)).collect(),
            (0..=last_a).map(|a| (a, last_b)).collect(),
            (0..=last_b).map(|b| (0, b)).collect(),
            (0..=last_b).map(|b| (last_a, b)).collect(),
        ];
        for edge in edges {
            let first = vertices.len() as u32;
            for &(a, b) in &edge {
                let (i, j) = fine(a, b);
                vertices.push(vertex(i, j, depth));
            }
            for k in 0..edge.len().saturating_sub(1) {
                let (t0, t1) = (at(edge[k].0, edge[k].1), at(edge[k + 1].0, edge[k + 1].1));
                let (b0, b1) = (first + k as u32, first + k as u32 + 1);
                indices.extend_from_slice(&[t0, b0, t1, t1, b0, b1, t0, t1, b0, t1, b1, b0]);
            }
        }
    }
}

/// Where streamed chunks go: the renderer, or a recorder in tests.
pub trait ChunkSink {
    /// Bytes that can still be written before the next frame.
    fn bytes_left(&self) -> usize;
    /// Replaces the contents of a slot.
    ///
    /// # Errors
    /// Whatever the sink rejects.
    fn write(
        &mut self,
        slot: MeshId,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Result<(), RenderError>;
}

impl ChunkSink for Renderer {
    fn bytes_left(&self) -> usize {
        self.stream_bytes_left()
    }

    fn write(
        &mut self,
        slot: MeshId,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Result<(), RenderError> {
        self.write_mesh_slot(slot, vertices, indices)
    }
}

/// What one [`TerrainStream::update`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamStats {
    /// Chunks built and written this frame.
    pub built: usize,
    /// Bytes written this frame.
    pub bytes: usize,
    /// Chunks drawn.
    pub drawn: usize,
    /// Chunks resident in the slots.
    pub resident: usize,
    /// Chunks wanted but not resident yet.
    pub waiting: usize,
    /// Wall time of the update, microseconds.
    pub micros: u64,
}

/// Streams the chunks of a [`WorldTerrain`] into renderer slots.
pub struct TerrainStream {
    terrain: Arc<WorldTerrain>,
    slots: Vec<MeshId>,
    /// Node held by each slot.
    owner: Vec<Option<NodeKey>>,
    /// Frame each slot was last needed.
    used: Vec<u64>,
    resident: BTreeMap<NodeKey, usize>,
    frame: u64,
    draw: Vec<NodeKey>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    last: StreamStats,
}

/// The six frustum planes of a clip-from-world matrix (`n . p + w >= 0`).
fn frustum(m: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2, r2]
}

fn in_frustum(planes: &[Vec4; 6], lo: Vec3, hi: Vec3) -> bool {
    let (c, e) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    planes.iter().all(|p| {
        let n = p.truncate();
        n.dot(c) + p.w + n.abs().dot(e) >= 0.0
    })
}

impl TerrainStream {
    /// A stream over `terrain` writing into `slots` (from
    /// [`Renderer::reserve_mesh_slots`] with [`SLOT_VERTICES`] and
    /// [`SLOT_INDICES`]). Nothing is resident yet.
    #[must_use]
    pub fn new(terrain: Arc<WorldTerrain>, slots: Vec<MeshId>) -> Self {
        let n = slots.len();
        Self {
            terrain,
            slots,
            owner: vec![None; n],
            used: vec![0; n],
            resident: BTreeMap::new(),
            frame: 0,
            draw: Vec::new(),
            vertices: Vec::with_capacity(SLOT_VERTICES as usize),
            indices: Vec::with_capacity(SLOT_INDICES as usize),
            last: StreamStats::default(),
        }
    }

    /// Reserves the slots in `renderer` and starts a stream.
    ///
    /// # Errors
    /// [`RenderError`] of the reservation (a full arena).
    pub fn with_renderer(
        terrain: Arc<WorldTerrain>,
        renderer: &mut Renderer,
    ) -> Result<Self, RenderError> {
        let slots = renderer.reserve_mesh_slots(SLOT_COUNT, SLOT_VERTICES, SLOT_INDICES)?;
        Ok(Self::new(terrain, slots))
    }

    /// The terrain streamed.
    #[must_use]
    pub fn terrain(&self) -> &WorldTerrain {
        &self.terrain
    }

    /// Whether `key` is fine enough seen from `eye`.
    fn fine_enough(&self, key: NodeKey, eye: Vec3) -> bool {
        key.level == 0 || self.projected_error(key, eye) <= PIXEL_ERROR
    }

    /// Error of a node in pixels, seen from `eye`.
    #[must_use]
    pub fn projected_error(&self, key: NodeKey, eye: Vec3) -> f32 {
        let (lo, hi) = self.terrain.bounds(key);
        let d = (eye.clamp(lo, hi) - eye).length().max(0.01);
        self.terrain.error(key) * PROJECTION_SCALE / d
    }

    fn touch(&mut self, key: NodeKey) -> bool {
        match self.resident.get(&key) {
            Some(&slot) => {
                self.used[slot] = self.frame;
                true
            }
            None => false,
        }
    }

    /// Picks this frame's chunks for the eye at `eye`, builds missing ones
    /// within the budgets and writes them to `sink`. `view_proj` (clip from
    /// world) only orders the loads: chunks in the view first.
    ///
    /// # Errors
    /// A [`RenderError`] of the sink other than a full budget.
    pub fn update(
        &mut self,
        eye: Vec3,
        view_proj: &Mat4,
        sink: &mut impl ChunkSink,
    ) -> Result<StreamStats, RenderError> {
        let start = Instant::now();
        self.frame += 1;
        let planes = frustum(view_proj);
        let mut draw = Vec::new();
        // (priority, node) of the chunks to build.
        let mut wanted: Vec<(f32, NodeKey)> = Vec::new();
        let root = self.terrain.root();
        if self.touch(root) {
            let mut stack = vec![root];
            while let Some(key) = stack.pop() {
                if self.fine_enough(key, eye) {
                    draw.push(key);
                    continue;
                }
                let children = self.terrain.children(key);
                let mut all = true;
                for &c in &children {
                    if !self.touch(c) {
                        all = false;
                        let (lo, hi) = self.terrain.bounds(c);
                        let seen = if in_frustum(&planes, lo, hi) {
                            1.0
                        } else {
                            0.25
                        };
                        wanted.push((self.projected_error(key, eye) * seen, c));
                    }
                }
                if all {
                    stack.extend(children);
                } else {
                    draw.push(key);
                }
            }
        } else {
            wanted.push((f32::MAX, root));
        }
        wanted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut stats = StreamStats {
            waiting: wanted.len(),
            ..StreamStats::default()
        };
        for &(_, key) in &wanted {
            if stats.built >= MAX_BUILDS_PER_FRAME || sink.bytes_left() < CHUNK_BYTES {
                break;
            }
            let Some(slot) = self.free_slot() else {
                break;
            };
            self.terrain
                .build_chunk(key, &mut self.vertices, &mut self.indices);
            match sink.write(self.slots[slot], &self.vertices, &self.indices) {
                Ok(()) => {}
                Err(RenderError::StreamBudget { .. }) => break,
                Err(e) => return Err(e),
            }
            if let Some(old) = self.owner[slot].replace(key) {
                self.resident.remove(&old);
            }
            self.resident.insert(key, slot);
            self.used[slot] = self.frame;
            stats.built += 1;
            stats.waiting -= 1;
            stats.bytes += self.vertices.len() * 20 + self.indices.len() * 4;
        }
        stats.drawn = draw.len();
        stats.resident = self.resident.len();
        self.draw = draw;
        stats.micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.last = stats;
        Ok(stats)
    }

    /// A free slot, or the least recently needed one not needed this frame
    /// (never the root's).
    fn free_slot(&self) -> Option<usize> {
        if let Some(s) = self.owner.iter().position(Option::is_none) {
            return Some(s);
        }
        let root = self.terrain.root();
        (0..self.slots.len())
            .filter(|&s| self.used[s] < self.frame && self.owner[s] != Some(root))
            .min_by_key(|&s| (self.used[s], s))
    }

    /// Nodes drawn by the last update.
    #[must_use]
    pub fn drawn(&self) -> &[NodeKey] {
        &self.draw
    }

    /// Stats of the last update.
    #[must_use]
    pub fn last_stats(&self) -> StreamStats {
        self.last
    }

    /// Whether every wanted chunk was resident at the last update.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.frame > 0 && self.last.waiting == 0
    }

    /// The static instances of the chunks drawn by the last update.
    #[must_use]
    pub fn instances(&self) -> Vec<Instance> {
        self.draw
            .iter()
            .filter_map(|k| self.resident.get(k))
            .map(|&slot| Instance::new(self.slots[slot], 0, Mat4::IDENTITY))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_mapfmt::FarField;

    /// A 512 x 384 m world of rolling hills (8 m far cells) around a 64 x 48 m
    /// bumpy terrain with 2 m cells.
    pub(crate) fn world() -> (World, Terrain) {
        let (nx, nz) = (65u16, 49u16);
        let mut heights = Vec::new();
        for j in 0..nz {
            for i in 0..nx {
                let (x, z) = (f32::from(i) * 8.0, f32::from(j) * 8.0);
                let h = 30.0 * (x / 90.0).sin() * (z / 70.0).cos() + 0.05 * x;
                #[allow(clippy::cast_possible_truncation)]
                heights.push((h * 10.0).round() as i32);
            }
        }
        let world = World {
            extent: [-256.0, -192.0, 256.0, 192.0],
            playable: [-40.0, -30.0, 40.0, 30.0],
            far: FarField {
                cell: 8,
                size: [nx, nz],
                heights,
                paint: Vec::new(),
            },
            snow_line: Some(20.0),
        };
        let mut t = Terrain::flat([-64, -48], 4, [33, 25], 0);
        for j in 0..25 {
            for i in 0..33 {
                let k = t.index(i, j);
                #[allow(clippy::cast_possible_truncation)]
                let h = (150.0 * ((i as f32) * 0.4).sin() + 80.0 * ((j as f32) * 0.3).cos()) as i16;
                t.heights[k] = h;
            }
        }
        t.paint = vec![5; 33 * 25];
        (world, t)
    }

    fn sample() -> WorldTerrain {
        let (w, t) = world();
        WorldTerrain::sample(&w, Some(&t), 59, Some(-5.0))
    }

    #[test]
    fn the_fine_grid_is_the_playable_terrain() {
        let (w, t) = world();
        let wt = WorldTerrain::sample(&w, Some(&t), 59, None);
        assert_eq!(wt.cell_m(), 2.0);
        let (nx, nz) = wt.dims();
        assert!(nx >= 257 && nz >= 193, "{nx} x {nz}");
        // Every terrain sample is a fine sample with the same height, and the
        // same normal inside the grid (on its border the world goes on).
        for j in 0..25 {
            for i in 0..33 {
                let p = t.sample_pos(i, j);
                let fi = ((p.x - wt.origin[0]) / 2.0).round() as usize;
                let fj = ((p.z - wt.origin[1]) / 2.0).round() as usize;
                assert!((wt.pos(fi, fj) - p).length() < 1e-4);
                if (1..32).contains(&i) && (1..24).contains(&j) {
                    assert!((wt.normal(fi, fj) - t.normal(i, j)).length() < 1e-4);
                }
            }
        }
        // Painted dirt inside the terrain: the dirt weight is full.
        let p = t.sample_pos(10, 10);
        let fi = ((p.x - wt.origin[0]) / 2.0).round() as usize;
        let fj = ((p.z - wt.origin[1]) / 2.0).round() as usize;
        assert_eq!(wt.splat[fj * nx + fi][0], 255);
    }

    #[test]
    fn errors_grow_with_the_level_and_bound_the_mesh() {
        let wt = sample();
        assert!(wt.level_count() >= 4);
        let root = wt.root();
        assert!(wt.error(root) > 0.1);
        for level in 1..wt.level_count() {
            let lv = &wt.levels[level];
            for z in 0..lv.nodes_z {
                for x in 0..lv.nodes_x {
                    let key = NodeKey {
                        level: level as u8,
                        x: x as u16,
                        z: z as u16,
                    };
                    for c in wt.children(key) {
                        assert!(wt.error(c) <= wt.error(key));
                    }
                }
            }
        }
        // Level 0 is the fine surface itself.
        let leaf = NodeKey {
            level: 0,
            x: 1,
            z: 1,
        };
        assert_eq!(wt.error(leaf), 0.0);
    }

    #[test]
    fn chunks_fit_their_slot_and_face_up() {
        let wt = sample();
        let (mut v, mut i) = (Vec::new(), Vec::new());
        for key in [
            wt.root(),
            NodeKey {
                level: 0,
                x: 0,
                z: 0,
            },
            NodeKey {
                level: 1,
                x: 3,
                z: 2,
            },
        ] {
            assert!(wt.exists(key), "{key:?}");
            wt.build_chunk(key, &mut v, &mut i);
            assert!(v.len() <= SLOT_VERTICES as usize && i.len() <= SLOT_INDICES as usize);
            assert!(i.iter().all(|&k| (k as usize) < v.len()));
            let surface = CHUNK_CELLS * CHUNK_CELLS * 6;
            for t in i[..surface.min(i.len())].chunks(3) {
                let p = |k: usize| Vec3::from_array(v[t[k] as usize].position);
                let n = (p(1) - p(0)).cross(p(2) - p(0));
                assert!(n.y >= 0.0, "surface triangle faces down");
            }
            for vert in &v {
                assert_eq!(vert.color_material[3], 59);
                assert!(vert.normal[3] >= 0);
            }
        }
        // The last row of chunks overhangs the grid: no triangle beyond it.
        let (nx, _) = wt.dims();
        let edge = NodeKey {
            level: 0,
            x: ((nx - 2) / CHUNK_CELLS) as u16,
            z: 0,
        };
        wt.build_chunk(edge, &mut v, &mut i);
        let max_x = v.iter().map(|p| p.position[0]).fold(f32::MIN, f32::max);
        assert!(max_x <= wt.origin[0] + (nx - 1) as f32 * 2.0 + 1e-3);
    }

    /// A sink that accepts everything up to a byte budget per frame.
    struct Recorder {
        left: usize,
        writes: usize,
        slots: std::collections::BTreeSet<MeshId>,
    }

    impl ChunkSink for Recorder {
        fn bytes_left(&self) -> usize {
            self.left
        }
        fn write(&mut self, slot: MeshId, v: &[Vertex], i: &[u32]) -> Result<(), RenderError> {
            let bytes = v.len() * 20 + i.len() * 4;
            if bytes > self.left {
                return Err(RenderError::StreamBudget {
                    bytes,
                    left: self.left,
                });
            }
            self.left -= bytes;
            self.writes += 1;
            self.slots.insert(slot);
            Ok(())
        }
    }

    fn view(eye: Vec3, yaw: f32) -> Mat4 {
        let dir = Vec3::new(yaw.sin(), -0.2, -yaw.cos());
        let v = faset_render::glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
        faset_render::perspective_reverse_z(75f32.to_radians(), 16.0 / 9.0, 0.1) * v
    }

    /// Area of the drawn nodes in fine cells, and whether two overlap.
    fn coverage(wt: &WorldTerrain, drawn: &[NodeKey]) -> (usize, bool) {
        let mut cells = std::collections::HashSet::new();
        let mut overlap = false;
        let (nx, nz) = wt.dims();
        for &k in drawn {
            let (s, i0, j0) = WorldTerrain::span(k);
            for j in j0..(j0 + CHUNK_CELLS * s).min(nz - 1) {
                for i in i0..(i0 + CHUNK_CELLS * s).min(nx - 1) {
                    overlap |= !cells.insert((i, j));
                }
            }
        }
        (cells.len(), overlap)
    }

    #[test]
    fn streaming_is_bounded_and_never_leaves_holes() {
        let wt = Arc::new(sample());
        let slots: Vec<MeshId> = (0..SLOT_COUNT).map(|k| MeshId(100 + k)).collect();
        let mut stream = TerrainStream::new(wt.clone(), slots);
        let mut sink = Recorder {
            left: 0,
            writes: 0,
            slots: Default::default(),
        };
        let (nx, nz) = wt.dims();
        let total = (nx - 1) * (nz - 1);
        // A flight across the world and a fast turn on the spot.
        let mut frames = 0;
        let mut settled_at = None;
        for step in 0..600 {
            let t = step as f32 / 600.0;
            let eye = if step < 400 {
                Vec3::new(-240.0 + 480.0 * t * 1.5, 25.0, -150.0 + 300.0 * t * 1.5)
            } else {
                Vec3::new(0.0, 12.0, 0.0)
            };
            let yaw = if step < 400 { 0.8 } else { step as f32 * 0.3 };
            sink.left = faset_render::STREAM_BYTES_PER_FRAME;
            let stats = stream.update(eye, &view(eye, yaw), &mut sink).unwrap();
            frames += 1;
            assert!(stats.built <= MAX_BUILDS_PER_FRAME);
            assert!(stats.bytes <= MAX_BUILDS_PER_FRAME * CHUNK_BYTES);
            assert!(stats.resident <= SLOT_COUNT as usize);
            if step > 0 {
                let (area, overlap) = coverage(&wt, stream.drawn());
                assert!(!overlap, "frame {step}: chunks overlap");
                assert_eq!(area, total, "frame {step}: hole in the ground");
            }
            if step >= 400 && settled_at.is_none() && stream.is_settled() {
                settled_at = Some(step);
            }
            if step > 420 && settled_at.is_some() {
                // Turning on the spot loads nothing once settled.
                assert_eq!(stats.built, 0, "frame {step} built while turning");
            }
            // Debug builds are slow: a loose bound that still catches an
            // algorithmic regression (release: well under a millisecond).
            let limit = if cfg!(debug_assertions) {
                250_000
            } else {
                4_000
            };
            assert!(stats.micros < limit, "frame {step}: {} us", stats.micros);
        }
        assert!(settled_at.is_some(), "never settled at the spot");
        assert!(sink.slots.len() <= SLOT_COUNT as usize);
        assert!(frames == 600 && sink.writes > 20);
        // Near the eye the finest level is drawn.
        assert!(stream.drawn().iter().any(|k| k.level == 0));
        assert!(stream.instances().len() == stream.drawn().len());
    }

    #[test]
    fn skirts_cover_the_cracks_between_drawn_levels() {
        let wt = Arc::new(sample());
        let slots: Vec<MeshId> = (0..SLOT_COUNT).map(MeshId).collect();
        let mut stream = TerrainStream::new(wt.clone(), slots);
        let mut sink = Recorder {
            left: 0,
            writes: 0,
            slots: Default::default(),
        };
        for eye in [Vec3::new(0.0, 4.0, 0.0), Vec3::new(-200.0, 40.0, 120.0)] {
            for _ in 0..80 {
                sink.left = usize::MAX;
                stream.update(eye, &view(eye, 0.0), &mut sink).unwrap();
            }
            assert!(stream.is_settled());
            let drawn = stream.drawn().to_vec();
            // Settled, every drawn chunk is within the error budget: a level
            // switch moves the ground by at most about a pixel.
            for &k in &drawn {
                assert!(k.level == 0 || stream.projected_error(k, eye) <= PIXEL_ERROR);
            }
            let levels: std::collections::BTreeSet<u8> = drawn.iter().map(|k| k.level).collect();
            assert!(levels.len() >= 2, "{levels:?}");
            // Map every fine sample on a chunk edge to the chunks holding it.
            let mut at: BTreeMap<(usize, usize), Vec<NodeKey>> = BTreeMap::new();
            let (nx, nz) = wt.dims();
            for &k in &drawn {
                let (s, i0, j0) = WorldTerrain::span(k);
                let (i1, j1) = (
                    (i0 + CHUNK_CELLS * s).min(nx - 1),
                    (j0 + CHUNK_CELLS * s).min(nz - 1),
                );
                for i in i0..=i1 {
                    at.entry((i, j0)).or_default().push(k);
                    at.entry((i, j1)).or_default().push(k);
                }
                for j in j0..=j1 {
                    at.entry((i0, j)).or_default().push(k);
                    at.entry((i1, j)).or_default().push(k);
                }
            }
            for ((i, j), keys) in at {
                let hs: Vec<(f32, NodeKey)> =
                    keys.iter().map(|&k| (wt.node_height(k, i, j), k)).collect();
                for &(ha, ka) in &hs {
                    for &(hb, _) in &hs {
                        // The higher edge's skirt reaches the lower edge.
                        if ha > hb {
                            assert!(
                                ha - hb <= wt.skirt_depth(ka) + 1e-4,
                                "gap {} at ({i}, {j}) under {ka:?}",
                                ha - hb
                            );
                        }
                    }
                }
            }
        }
    }
}
