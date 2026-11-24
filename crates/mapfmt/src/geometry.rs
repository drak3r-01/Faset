//! CPU geometry of a map: flat-shaded mesh chunks and per-vertex ambient
//! occlusion. Pure data, no renderer and no physics types.
//!
//! [`build_meshes`] expects a map already expanded ([`crate::expand`]).
//! Faces of boxes are cut where another box touches or contains them, so a
//! face shared by two touching boxes produces no triangle. Faces of the other
//! brushes (wedges, cylinders, prisms, stairs) are always emitted. Polygons
//! are split on the 16 m block planes, one [`MeshChunk`] per block. The
//! terrain (lot 6.10) adds smooth-shaded triangles in the block of their
//! centroid, after the brush faces, plus vertical skirts on its borders.
//!
//! Decision: materials are names in the file; [`material_style`] maps a name
//! to a renderer material id and a base colour. Unknown names give id 0 and
//! a neutral grey.

use std::collections::BTreeMap;

use faset_core::glam::Vec3;

use crate::schema::{grid_to_metres, Axis, Brush, Dir, GridPos, MapFile};
use crate::terrain::Terrain;

/// Edge of a mesh block in metres.
pub const BLOCK_SIZE_M: f32 = 16.0;

/// Material id and base colour of a face.
type Style = (u8, [u8; 3]);

/// One mesh vertex, 20 bytes, layout-identical to the renderer's vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Unit face normal, snorm8 (w = 0).
    pub normal: [i8; 4],
    /// r, g, b (linear, ambient occlusion included once baked), material id.
    pub color_material: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 20);

impl Vertex {
    /// The unit normal decoded from its snorm8 form.
    #[must_use]
    pub fn normal_vec(&self) -> Vec3 {
        let n = self.normal;
        Vec3::new(f32::from(n[0]), f32::from(n[1]), f32::from(n[2])).normalize_or_zero()
    }
}

#[allow(clippy::cast_possible_truncation)]
fn snorm8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// Triangles of one 16 m block. Triangles are counter-clockwise seen from
/// outside and indices are local to `vertices`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshChunk {
    /// Block index: floor(position / 16 m) on each axis.
    pub block: [i32; 3],
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Material names the map format knows, with the renderer material id they
/// paint (ids of `render/data/materials.ron`) and a base colour that tints
/// the pattern (white leaves it as is).
pub const MATERIAL_NAMES: [(&str, u8); 42] = [
    ("brick", 14),
    ("wood", 15),
    ("grass", 16),
    ("sand", 17),
    ("stone", 18),
    ("concrete", 19),
    ("metal", 20),
    ("water", 21),
    ("tiles", 22),
    ("dirt", 23),
    ("snow", 24),
    ("crate", 25),
    ("hazard", 26),
    ("leaves", 27),
    ("brick_sand", 28),
    ("wood_light", 29),
    ("wood_dark", 30),
    ("sandstone", 31),
    ("mossy_stone", 32),
    ("concrete_white", 33),
    ("metal_orange", 34),
    ("container_blue", 35),
    ("container_red", 36),
    ("metal_green", 37),
    ("savanna", 38),
    ("lagoon", 39),
    ("sand_white", 40),
    ("basalt", 41),
    ("tiles_teal", 42),
    ("asphalt", 43),
    ("palm_leaves", 44),
    ("ice", 45),
    ("deep_water", 46),
    ("brick_slate", 47),
    ("wood_white", 48),
    ("clay", 49),
    ("lamp_warm", 50),
    ("lamp_white", 51),
    ("lamp_red", 52),
    ("lamp_green", 53),
    ("checker", 3),
    ("grid", 5),
];

/// Renderer material id and base colour (linear RGB) of a material name.
/// Lava, emissive and hologram keep their classic patterns. Unknown names
/// give id 0 and a neutral grey.
#[must_use]
pub fn material_style(name: &str) -> (u8, [u8; 3]) {
    if let Some((_, id)) = MATERIAL_NAMES.iter().find(|(n, _)| *n == name) {
        return (*id, [255, 255, 255]);
    }
    match name {
        "lava" => (11, [255, 255, 255]),
        "emissive" => (12, [255, 255, 255]),
        "hologram" => (13, [255, 255, 255]),
        _ => (0, [120, 120, 120]),
    }
}

/// A convex polygon, counter-clockwise seen from outside.
struct Poly {
    pts: Vec<Vec3>,
    normal: Vec3,
    style: Style,
}

/// Builds a polygon from unordered coplanar points; `hint` points outward.
#[allow(clippy::cast_precision_loss)]
fn poly(pts: &[Vec3], hint: Vec3, style: Style) -> Option<Poly> {
    if pts.len() < 3 {
        return None;
    }
    let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    // Normal from the pair of edges with the largest cross product.
    let e1 = pts[1] - pts[0];
    let mut n = Vec3::ZERO;
    for p in &pts[2..] {
        let x = e1.cross(*p - pts[0]);
        if x.length_squared() > n.length_squared() {
            n = x;
        }
    }
    if n.length_squared() < 1e-12 {
        return None;
    }
    let mut n = n.normalize();
    if n.dot(hint) < 0.0 {
        n = -n;
    }
    let u = (pts[0] - c).normalize_or_zero();
    let v = n.cross(u);
    let mut sorted: Vec<(f32, Vec3)> = pts
        .iter()
        .map(|p| {
            let d = *p - c;
            (d.dot(v).atan2(d.dot(u)), *p)
        })
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(Poly {
        pts: sorted.into_iter().map(|(_, p)| p).collect(),
        normal: n,
        style,
    })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Maps unit coordinates (u along the rise direction, v across it, h up) of
/// a wedge or stairs box to metres.
fn ramp_frame(min: GridPos, max: GridPos, rise: Dir) -> impl Fn(f32, f32, f32) -> Vec3 {
    let (lo, hi) = (grid_to_metres(min), grid_to_metres(max));
    move |u, v, h| {
        let y = lerp(lo.y, hi.y, h);
        match rise {
            Dir::PosX => Vec3::new(lerp(lo.x, hi.x, u), y, lerp(lo.z, hi.z, v)),
            Dir::NegX => Vec3::new(lerp(hi.x, lo.x, u), y, lerp(lo.z, hi.z, v)),
            Dir::PosZ => Vec3::new(lerp(lo.x, hi.x, v), y, lerp(lo.z, hi.z, u)),
            Dir::NegZ => Vec3::new(lerp(lo.x, hi.x, v), y, lerp(hi.z, lo.z, u)),
        }
    }
}

fn axis_index(a: Axis) -> usize {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

fn axis_vec(i: usize) -> Vec3 {
    let mut v = Vec3::ZERO;
    v[i] = 1.0;
    v
}

/// Vertices of the convex volume of a brush, as used for collision
/// (`Stairs` gives its ramp). Boxes give their 8 corners.
#[must_use]
pub fn brush_hull_points(brush: &Brush) -> Vec<Vec3> {
    match brush {
        Brush::Box { min, max, .. } => {
            let (lo, hi) = (grid_to_metres(*min), grid_to_metres(*max));
            (0..8)
                .map(|i| {
                    Vec3::new(
                        if i & 1 == 0 { lo.x } else { hi.x },
                        if i & 2 == 0 { lo.y } else { hi.y },
                        if i & 4 == 0 { lo.z } else { hi.z },
                    )
                })
                .collect()
        }
        Brush::Wedge { min, max, rise, .. } | Brush::Stairs { min, max, rise, .. } => {
            let f = ramp_frame(*min, *max, *rise);
            vec![
                f(0.0, 0.0, 0.0),
                f(1.0, 0.0, 0.0),
                f(0.0, 1.0, 0.0),
                f(1.0, 1.0, 0.0),
                f(1.0, 0.0, 1.0),
                f(1.0, 1.0, 1.0),
            ]
        }
        Brush::Cylinder {
            min,
            max,
            axis,
            sides,
            ..
        } => {
            let (lo, hi) = cylinder_rings(*min, *max, *axis, *sides);
            [lo, hi].concat()
        }
        Brush::Prism {
            points,
            y_min,
            y_max,
            ..
        } => {
            let (lo, hi) = prism_rings(points, *y_min, *y_max);
            [lo, hi].concat()
        }
    }
}

/// The two rings (low end, high end along the axis) of a cylinder.
#[allow(clippy::cast_precision_loss)]
fn cylinder_rings(min: GridPos, max: GridPos, axis: Axis, sides: u8) -> (Vec<Vec3>, Vec<Vec3>) {
    let (lo, hi) = (grid_to_metres(min), grid_to_metres(max));
    let a = axis_index(axis);
    let (p, q) = ((a + 1) % 3, (a + 2) % 3);
    let centre = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let ring = |end: f32| {
        (0..sides)
            .map(|k| {
                let t = std::f32::consts::TAU * f32::from(k) / f32::from(sides);
                let mut v = centre;
                v[a] = end;
                v[p] += half[p] * t.cos();
                v[q] += half[q] * t.sin();
                v
            })
            .collect::<Vec<_>>()
    };
    (ring(lo[a]), ring(hi[a]))
}

/// The bottom and top rings of a prism.
fn prism_rings(points: &[[i32; 2]], y_min: i32, y_max: i32) -> (Vec<Vec3>, Vec<Vec3>) {
    let (y0, y1) = (
        grid_to_metres([0, y_min, 0]).y,
        grid_to_metres([0, y_max, 0]).y,
    );
    let at = |p: &[i32; 2], y: f32| {
        let m = grid_to_metres([p[0], 0, p[1]]);
        Vec3::new(m.x, y, m.z)
    };
    (
        points.iter().map(|p| at(p, y0)).collect(),
        points.iter().map(|p| at(p, y1)).collect(),
    )
}

/// Polygons of every brush that is not a box.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
fn other_polys(brush: &Brush, out: &mut Vec<Poly>) {
    match brush {
        Brush::Box { .. } => {}
        Brush::Wedge {
            min,
            max,
            rise,
            material,
        } => {
            let s = material_style(material);
            let f = ramp_frame(*min, *max, *rise);
            let (a, b, c, d, e, g) = (
                f(0.0, 0.0, 0.0),
                f(1.0, 0.0, 0.0),
                f(0.0, 1.0, 0.0),
                f(1.0, 1.0, 0.0),
                f(1.0, 0.0, 1.0),
                f(1.0, 1.0, 1.0),
            );
            let centre = (a + b + c + d + e + g) / 6.0;
            for face in [
                &[a, b, d, c][..],
                &[b, d, g, e],
                &[a, c, g, e],
                &[a, b, e],
                &[c, d, g],
            ] {
                let fc = face.iter().copied().sum::<Vec3>() / face.len() as f32;
                out.extend(poly(face, fc - centre, s));
            }
        }
        Brush::Stairs {
            min,
            max,
            rise,
            steps,
            material,
        } => {
            let s = material_style(material);
            let f = ramp_frame(*min, *max, *rise);
            let n = f32::from(*steps);
            let forward = (f(1.0, 0.0, 0.0) - f(0.0, 0.0, 0.0)).normalize_or_zero();
            let side = (f(0.0, 1.0, 0.0) - f(0.0, 0.0, 0.0)).normalize_or_zero();
            let mut quad = |p: [Vec3; 4], hint: Vec3| out.extend(poly(&p, hint, s));
            quad(
                [
                    f(0.0, 0.0, 0.0),
                    f(1.0, 0.0, 0.0),
                    f(1.0, 1.0, 0.0),
                    f(0.0, 1.0, 0.0),
                ],
                Vec3::NEG_Y,
            );
            quad(
                [
                    f(1.0, 0.0, 0.0),
                    f(1.0, 1.0, 0.0),
                    f(1.0, 1.0, 1.0),
                    f(1.0, 0.0, 1.0),
                ],
                forward,
            );
            for k in 0..*steps {
                let (u0, u1) = (f32::from(k) / n, f32::from(k + 1) / n);
                // Step k tops out at height u1 (the rise is linear).
                for (v, hint) in [(0.0, -side), (1.0, side)] {
                    quad(
                        [f(u0, v, 0.0), f(u1, v, 0.0), f(u1, v, u1), f(u0, v, u1)],
                        hint,
                    );
                }
                quad(
                    [
                        f(u0, 0.0, u1),
                        f(u1, 0.0, u1),
                        f(u1, 1.0, u1),
                        f(u0, 1.0, u1),
                    ],
                    Vec3::Y,
                );
                quad(
                    [
                        f(u0, 0.0, u0),
                        f(u0, 1.0, u0),
                        f(u0, 1.0, u1),
                        f(u0, 0.0, u1),
                    ],
                    -forward,
                );
            }
        }
        Brush::Cylinder {
            min,
            max,
            axis,
            sides,
            material,
        } => {
            let s = material_style(material);
            let (lo, hi) = cylinder_rings(*min, *max, *axis, *sides);
            let a = axis_vec(axis_index(*axis));
            out.extend(poly(&lo, -a, s));
            out.extend(poly(&hi, a, s));
            ring_sides(&lo, &hi, s, out);
        }
        Brush::Prism {
            points,
            y_min,
            y_max,
            material,
        } => {
            let s = material_style(material);
            let (lo, hi) = prism_rings(points, *y_min, *y_max);
            out.extend(poly(&lo, Vec3::NEG_Y, s));
            out.extend(poly(&hi, Vec3::Y, s));
            ring_sides(&lo, &hi, s, out);
        }
    }
}

/// Side quads between two matching rings of a convex solid.
#[allow(clippy::cast_precision_loss)]
fn ring_sides(lo: &[Vec3], hi: &[Vec3], s: Style, out: &mut Vec<Poly>) {
    let centre = lo.iter().chain(hi).copied().sum::<Vec3>() / (lo.len() + hi.len()) as f32;
    for k in 0..lo.len() {
        let j = (k + 1) % lo.len();
        let q = [lo[k], lo[j], hi[j], hi[k]];
        let fc = q.iter().copied().sum::<Vec3>() / 4.0;
        out.extend(poly(&q, fc - centre, s));
    }
}

/// A rectangle on the plane of a box face: ranges on the two other axes.
type Rect = [(i32, i32); 2];

/// `r` minus `cut`; up to four rectangles.
fn subtract(r: Rect, cut: Rect, out: &mut Vec<Rect>) {
    let ib = (r[0].0.max(cut[0].0), r[0].1.min(cut[0].1));
    let ic = (r[1].0.max(cut[1].0), r[1].1.min(cut[1].1));
    if ib.0 >= ib.1 || ic.0 >= ic.1 {
        out.push(r);
        return;
    }
    let mut push = |b: (i32, i32), c: (i32, i32)| {
        if b.0 < b.1 && c.0 < c.1 {
            out.push([b, c]);
        }
    };
    push((r[0].0, ib.0), r[1]);
    push((ib.1, r[0].1), r[1]);
    push(ib, (r[1].0, ic.0));
    push(ib, (ic.1, r[1].1));
}

type GridBox = (GridPos, GridPos, Style);

fn box_polys(boxes: &[GridBox], out: &mut Vec<Poly>) {
    for (i, (min, max, style)) in boxes.iter().enumerate() {
        for a in 0..3 {
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            for positive in [false, true] {
                let plane = if positive { max[a] } else { min[a] };
                let mut rects: Vec<Rect> = vec![[(min[b], max[b]), (min[c], max[c])]];
                for (j, (omin, omax, _)) in boxes.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    // Another box hides this face where it touches it from
                    // the outside or where the face is buried inside it.
                    let touching = if positive {
                        omin[a] == plane
                    } else {
                        omax[a] == plane
                    };
                    let inside = omin[a] < plane && plane < omax[a];
                    if !(touching || inside) {
                        continue;
                    }
                    let cut: Rect = [(omin[b], omax[b]), (omin[c], omax[c])];
                    let mut next = Vec::with_capacity(rects.len() + 3);
                    for r in rects {
                        subtract(r, cut, &mut next);
                    }
                    rects = next;
                    if rects.is_empty() {
                        break;
                    }
                }
                let normal = if positive { axis_vec(a) } else { -axis_vec(a) };
                for r in rects {
                    let at = |pb: i32, pc: i32| {
                        let mut g = [0; 3];
                        g[a] = plane;
                        g[b] = pb;
                        g[c] = pc;
                        grid_to_metres(g)
                    };
                    let pts = [
                        at(r[0].0, r[1].0),
                        at(r[0].1, r[1].0),
                        at(r[0].1, r[1].1),
                        at(r[0].0, r[1].1),
                    ];
                    out.extend(poly(&pts, normal, *style));
                }
            }
        }
    }
}

/// Clips `pts` to one side of the plane `p[axis] = at`.
fn clip(pts: &[Vec3], axis: usize, at: f32, below: bool) -> Vec<Vec3> {
    let side = |p: Vec3| if below { at - p[axis] } else { p[axis] - at };
    let mut out = Vec::with_capacity(pts.len() + 2);
    for (k, &p) in pts.iter().enumerate() {
        let q = pts[(k + 1) % pts.len()];
        let (sp, sq) = (side(p), side(q));
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp > 0.0 && sq < 0.0) || (sp < 0.0 && sq > 0.0) {
            let t = sp / (sp - sq);
            let mut m = p + (q - p) * t;
            m[axis] = at;
            out.push(m);
        }
    }
    out
}

/// Splits a convex polygon on the block planes of every axis.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn split_blocks(pts: Vec<Vec3>) -> Vec<Vec<Vec3>> {
    let mut pieces = vec![pts];
    for axis in 0..3 {
        let mut next = Vec::new();
        for piece in pieces {
            let (lo, hi) = piece.iter().fold((f32::MAX, f32::MIN), |(l, h), p| {
                (l.min(p[axis]), h.max(p[axis]))
            });
            let mut rest = piece;
            let mut k = (lo / BLOCK_SIZE_M).floor() as i32 + 1;
            while (k as f32) * BLOCK_SIZE_M < hi - 1e-4 {
                let at = k as f32 * BLOCK_SIZE_M;
                if at > lo + 1e-4 {
                    let below = clip(&rest, axis, at, true);
                    if below.len() >= 3 {
                        next.push(below);
                    }
                    rest = clip(&rest, axis, at, false);
                }
                k += 1;
            }
            if rest.len() >= 3 {
                next.push(rest);
            }
        }
        pieces = next;
    }
    pieces
}

#[allow(clippy::cast_possible_truncation)]
fn block_of(p: Vec3) -> [i32; 3] {
    [0, 1, 2].map(|i| (p[i] / BLOCK_SIZE_M).floor() as i32)
}

/// Builds the render meshes of an expanded map, one chunk per 16 m block,
/// ordered by block index.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn build_meshes(map: &MapFile) -> Vec<MeshChunk> {
    let mut boxes = Vec::new();
    let mut polys = Vec::new();
    for b in &map.brushes {
        match b {
            Brush::Box { min, max, material } => boxes.push((*min, *max, material_style(material))),
            other => other_polys(other, &mut polys),
        }
    }
    box_polys(&boxes, &mut polys);

    let mut chunks: BTreeMap<[i32; 3], MeshChunk> = BTreeMap::new();
    for p in polys {
        for piece in split_blocks(p.pts.clone()) {
            let area2: f32 = (1..piece.len() - 1)
                .map(|i| {
                    (piece[i] - piece[0])
                        .cross(piece[i + 1] - piece[0])
                        .length()
                })
                .sum();
            if area2 < 1e-6 {
                continue;
            }
            let centroid = piece.iter().copied().sum::<Vec3>() / piece.len() as f32;
            // A face lying on a block plane belongs to the block it faces away from.
            let block = block_of(centroid - p.normal * 1e-3);
            let chunk = chunks.entry(block).or_insert_with(|| MeshChunk {
                block,
                ..MeshChunk::default()
            });
            let base = chunk.vertices.len() as u32;
            let n = [
                snorm8(p.normal.x),
                snorm8(p.normal.y),
                snorm8(p.normal.z),
                0,
            ];
            for v in &piece {
                chunk.vertices.push(Vertex {
                    position: v.to_array(),
                    normal: n,
                    color_material: [p.style.1[0], p.style.1[1], p.style.1[2], p.style.0],
                });
            }
            for i in 1..piece.len() as u32 - 1 {
                chunk
                    .indices
                    .extend_from_slice(&[base, base + i, base + i + 1]);
            }
        }
    }
    if let Some(t) = &map.terrain {
        terrain_mesh(t, &map.theme.palette, &mut chunks);
    }
    chunks.into_values().collect()
}

/// Renderer material ids of the terrain grounds (`render/data/materials.ron`,
/// pattern `Terrain`): each blends its flat ground to a rock on steep slopes.
/// Index = paint value - 1 (grass, sand, rock, snow, dirt).
pub const TERRAIN_MATERIALS: [u8; 5] = [54, 55, 56, 57, 58];

/// Material id of a terrain sample: its paint, or for paint 0 (auto) the
/// ground of the map palette (snow, desert, nature, industrial; grass for
/// the others).
#[must_use]
pub fn terrain_material(palette: &str, paint: u8) -> u8 {
    let ground = match paint {
        0 => match palette {
            "desert" => 2,
            "snow" => 4,
            "industrial" | "urban" => 5,
            _ => 1,
        },
        p => p,
    };
    TERRAIN_MATERIALS[usize::from(ground.clamp(1, 5)) - 1]
}

/// Appends the terrain triangles (smooth normals, one vertex per sample and
/// chunk) and the border skirts to the chunks; a triangle belongs to the
/// block of its centroid.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn terrain_mesh(t: &Terrain, palette: &str, chunks: &mut BTreeMap<[i32; 3], MeshChunk>) {
    let (nx, nz) = t.dims();
    let mut shared: BTreeMap<([i32; 3], usize), u32> = BTreeMap::new();
    for tri in t.triangles() {
        let p = tri.map(|(i, j)| t.sample_pos(i, j));
        let block = block_of((p[0] + p[1] + p[2]) / 3.0);
        let chunk = chunks.entry(block).or_insert_with(|| MeshChunk {
            block,
            ..MeshChunk::default()
        });
        for (k, &(i, j)) in tri.iter().enumerate() {
            let index = *shared.entry((block, t.index(i, j))).or_insert_with(|| {
                let n = t.normal(i, j);
                chunk.vertices.push(Vertex {
                    position: p[k].to_array(),
                    normal: [snorm8(n.x), snorm8(n.y), snorm8(n.z), 0],
                    color_material: [255, 255, 255, terrain_material(palette, t.paint_at(i, j))],
                });
                chunk.vertices.len() as u32 - 1
            });
            chunk.indices.push(index);
        }
    }
    // Skirts: the four borders, from the surface down to the base.
    let base = t.base_y();
    let rock = terrain_material(palette, 3);
    // Border edges: two samples `(i, j)` and the outward normal.
    type Edge = ((usize, usize), (usize, usize), Vec3);
    let mut edges: Vec<Edge> = Vec::new();
    for i in 0..nx - 1 {
        edges.push(((i + 1, 0), (i, 0), Vec3::NEG_Z));
        edges.push(((i, nz - 1), (i + 1, nz - 1), Vec3::Z));
    }
    for j in 0..nz - 1 {
        edges.push(((0, j), (0, j + 1), Vec3::NEG_X));
        edges.push(((nx - 1, j + 1), (nx - 1, j), Vec3::X));
    }
    for (a, b, normal) in edges {
        let (pa, pb) = (t.sample_pos(a.0, a.1), t.sample_pos(b.0, b.1));
        let (qa, qb) = (Vec3::new(pa.x, base, pa.z), Vec3::new(pb.x, base, pb.z));
        let Some(face) = poly(&[pa, pb, qb, qa], normal, (rock, [255, 255, 255])) else {
            continue;
        };
        let centroid = face.pts.iter().copied().sum::<Vec3>() / face.pts.len() as f32;
        let block = block_of(centroid);
        let chunk = chunks.entry(block).or_insert_with(|| MeshChunk {
            block,
            ..MeshChunk::default()
        });
        let first = chunk.vertices.len() as u32;
        let n = [snorm8(normal.x), snorm8(normal.y), snorm8(normal.z), 0];
        for v in &face.pts {
            chunk.vertices.push(Vertex {
                position: v.to_array(),
                normal: n,
                color_material: [255, 255, 255, rock],
            });
        }
        for i in 1..face.pts.len() as u32 - 1 {
            chunk
                .indices
                .extend_from_slice(&[first, first + i, first + i + 1]);
        }
    }
}

/// Rays per vertex.
pub const AO_RAYS: u32 = 16;
/// Length of an occlusion ray in metres.
pub const AO_RAY_LENGTH_M: f32 = 1.5;
/// Darkest factor a fully enclosed vertex receives.
pub const AO_MIN: f32 = 0.4;
/// Offset of the ray origin along the normal, in metres.
const AO_BIAS_M: f32 = 0.02;

/// Ambient occlusion factor in [`AO_MIN`, 1] of a point on a surface:
/// 1 minus the share of [`AO_RAYS`] cosine-distributed rays of length
/// [`AO_RAY_LENGTH_M`] that `occluded(origin, direction, length)` reports hit.
/// Deterministic: the ray pattern is fixed.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn vertex_occlusion(
    position: Vec3,
    normal: Vec3,
    occluded: &impl Fn(Vec3, Vec3, f32) -> bool,
) -> f32 {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return 1.0;
    }
    let helper = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let t = n.cross(helper).normalize();
    let b = n.cross(t);
    let origin = position + n * AO_BIAS_M;
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    let mut hits = 0;
    for k in 0..AO_RAYS {
        let u = (k as f32 + 0.5) / AO_RAYS as f32;
        let (r, z) = (u.sqrt(), (1.0 - u).sqrt());
        let phi = golden * k as f32;
        let dir = t * (r * phi.cos()) + b * (r * phi.sin()) + n * z;
        if occluded(origin, dir, AO_RAY_LENGTH_M) {
            hits += 1;
        }
    }
    1.0 - (1.0 - AO_MIN) * hits as f32 / AO_RAYS as f32
}

/// Multiplies the ambient occlusion of every vertex into its RGB.
/// `occluded(origin, direction, max_distance)` is a ray test against the
/// solid geometry (see `faset_sim::map_collision::MapCollision::ray_cast`).
pub fn bake_occlusion(chunks: &mut [MeshChunk], occluded: impl Fn(Vec3, Vec3, f32) -> bool) {
    for chunk in chunks {
        for v in &mut chunk.vertices {
            let ao = vertex_occlusion(Vec3::from_array(v.position), v.normal_vec(), &occluded);
            for c in &mut v.color_material[..3] {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    *c = (f32::from(*c) * ao).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Meta, Symmetry, Theme};

    fn map(brushes: Vec<Brush>) -> MapFile {
        MapFile {
            format_version: 1,
            meta: Meta {
                name: "t".into(),
                author: String::new(),
                description: String::new(),
                modes: vec![],
                players: (1, 2),
            },
            theme: Theme {
                ambience: String::new(),
                sky: String::new(),
                weather: vec![],
                palette: String::new(),
            },
            physics: None,
            symmetry: Symmetry::None,
            prefabs: vec![],
            brushes,
            props: vec![],
            spawns: vec![],
            pickups: vec![],
            zones: vec![],
            lights: vec![],
            terrain: None,
        }
    }

    fn bx(min: GridPos, max: GridPos) -> Brush {
        Brush::Box {
            min,
            max,
            material: "concrete".into(),
        }
    }

    fn triangles(chunks: &[MeshChunk]) -> usize {
        chunks.iter().map(|c| c.indices.len() / 3).sum()
    }

    #[test]
    fn vertex_is_20_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 20);
    }

    #[test]
    fn lone_box_has_twelve_triangles() {
        let c = build_meshes(&map(vec![bx([0, 0, 0], [4, 4, 4])]));
        assert_eq!(c.len(), 1);
        assert_eq!(triangles(&c), 12);
        assert_eq!(c[0].vertices.len(), 24);
    }

    #[test]
    fn shared_face_has_no_triangles() {
        let c = build_meshes(&map(vec![
            bx([0, 0, 0], [4, 4, 4]),
            bx([4, 0, 0], [8, 4, 4]),
        ]));
        // Two boxes: 12 faces, the two touching ones are gone.
        assert_eq!(triangles(&c), 20);
        let on_plane = |c: &MeshChunk| {
            c.vertices
                .iter()
                .any(|v| (v.position[0] - 2.0).abs() < 1e-6 && v.normal[0] != 0)
        };
        assert!(
            !c.iter().any(on_plane),
            "no face remains on the shared plane"
        );
    }

    #[test]
    fn partially_covered_face_keeps_its_exposed_part() {
        let c = build_meshes(&map(vec![
            bx([0, 0, 0], [4, 4, 4]),
            bx([4, 0, 0], [8, 2, 4]),
        ]));
        // Box A's +X face is 2 x 2 m; the neighbour hides its lower half.
        let mut area = 0.0;
        for ch in &c {
            for t in ch.indices.chunks(3) {
                let v = |i: usize| &ch.vertices[t[i] as usize];
                if v(0).normal[0] == 127 && (v(0).position[0] - 2.0).abs() < 1e-6 {
                    let p = |i: usize| Vec3::from_array(v(i).position);
                    area += (p(1) - p(0)).cross(p(2) - p(0)).length() * 0.5;
                }
            }
        }
        assert!((area - 2.0).abs() < 1e-4, "{area}");
    }

    #[test]
    fn block_boundary_splits_chunks() {
        // 32 m long: crosses x = 16 m.
        let c = build_meshes(&map(vec![bx([0, 0, 0], [64, 2, 8])]));
        let blocks: Vec<_> = c.iter().map(|c| c.block).collect();
        assert_eq!(blocks, [[0, 0, 0], [1, 0, 0]]);
        // Two separate boxes in two blocks.
        let c = build_meshes(&map(vec![
            bx([0, 0, 0], [8, 2, 8]),
            bx([40, 0, 0], [48, 2, 8]),
        ]));
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].block, [1, 0, 0]);
    }

    #[test]
    fn negative_blocks_use_floor() {
        let c = build_meshes(&map(vec![bx([-8, 0, 0], [-2, 2, 2])]));
        assert_eq!(c[0].block, [-1, 0, 0]);
    }

    #[test]
    fn every_brush_kind_faces_outward() {
        let brushes = vec![
            bx([0, 0, 0], [4, 4, 4]),
            Brush::Wedge {
                min: [0, 0, 0],
                max: [6, 4, 4],
                rise: Dir::NegZ,
                material: "metal".into(),
            },
            Brush::Cylinder {
                min: [0, 0, 0],
                max: [6, 8, 6],
                axis: Axis::X,
                sides: 9,
                material: "metal".into(),
            },
            Brush::Prism {
                points: vec![[0, 0], [8, 0], [4, 6]],
                y_min: 0,
                y_max: 4,
                material: "metal".into(),
            },
        ];
        for b in brushes {
            let pts = brush_hull_points(&b);
            let centre = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
            let chunks = build_meshes(&map(vec![b.clone()]));
            assert!(triangles(&chunks) > 0);
            for ch in &chunks {
                for t in ch.indices.chunks(3) {
                    let p = |i: usize| Vec3::from_array(ch.vertices[t[i] as usize].position);
                    let n = (p(1) - p(0)).cross(p(2) - p(0)).normalize();
                    assert!(
                        n.dot(ch.vertices[t[0] as usize].normal_vec()) > 0.98,
                        "{b:?}"
                    );
                    assert!(n.dot(p(0) - centre) > 0.0, "{b:?} faces inward");
                }
            }
        }
    }

    #[test]
    fn stairs_show_steps() {
        let c = build_meshes(&map(vec![Brush::Stairs {
            min: [0, 0, 0],
            max: [4, 6, 8],
            rise: Dir::PosZ,
            steps: 3,
            material: "metal".into(),
        }]));
        let mut heights: Vec<i32> = c[0]
            .vertices
            .iter()
            .filter(|v| v.normal[1] == 127)
            .map(|v| (v.position[1] * 100.0).round() as i32)
            .collect();
        heights.sort_unstable();
        heights.dedup();
        assert_eq!(heights, [100, 200, 300]);
    }

    fn hill() -> Terrain {
        // 40 x 40 m, 2 m cells, a smooth bump of 3 m in the middle.
        let mut t = Terrain::flat([-40, -40], 4, [21, 21], 0);
        for j in 0..21 {
            for i in 0..21 {
                let p = t.sample_pos(i, j);
                let k = t.index(i, j);
                t.heights[k] = (300.0 * (-(p.x * p.x + p.z * p.z) / 60.0).exp()).round() as i16;
            }
        }
        t
    }

    #[test]
    fn terrain_mesh_is_smooth_chunked_and_faces_up() {
        let mut m = map(vec![]);
        m.theme.palette = "snow".into();
        m.terrain = Some(hill());
        let chunks = build_meshes(&m);
        // 400 cells, 2 triangles each, plus 80 skirt quads.
        assert_eq!(triangles(&chunks), 800 + 160);
        // 40 m spans several 16 m blocks.
        assert!(chunks.len() >= 9, "{}", chunks.len());
        let snow = terrain_material("snow", 0);
        assert_eq!(snow, TERRAIN_MATERIALS[3]);
        let mut smooth = 0;
        for ch in &chunks {
            for t in ch.indices.chunks(3) {
                let v = |i: usize| &ch.vertices[t[i] as usize];
                if v(0).color_material[3] != snow {
                    continue;
                }
                let p = |i: usize| Vec3::from_array(v(i).position);
                let n = (p(1) - p(0)).cross(p(2) - p(0)).normalize();
                assert!(n.y > 0.0, "terrain triangle faces down");
                // Vertex normals differ from the face normal: smooth shading.
                if (0..3).any(|i| v(i).normal_vec().dot(n) < 0.999) {
                    smooth += 1;
                }
            }
        }
        assert!(smooth > 100, "{smooth}");
        // Paint overrides the palette.
        let mut painted = hill();
        painted.paint = vec![2; painted.heights.len()];
        m.terrain = Some(painted);
        let chunks = build_meshes(&m);
        assert!(chunks
            .iter()
            .flat_map(|c| &c.vertices)
            .any(|v| v.color_material[3] == terrain_material("snow", 2)));
    }

    #[test]
    fn terrain_material_follows_the_palette() {
        assert_eq!(terrain_material("nature", 0), 54);
        assert_eq!(terrain_material("desert", 0), 55);
        assert_eq!(terrain_material("snow", 0), 57);
        assert_eq!(terrain_material("industrial", 0), 58);
        assert_eq!(terrain_material("desert", 3), 56);
    }

    #[test]
    fn occlusion_lies_in_range() {
        let open = vertex_occlusion(Vec3::ZERO, Vec3::Y, &|_, _, _| false);
        let closed = vertex_occlusion(Vec3::ZERO, Vec3::Y, &|_, _, _| true);
        assert!((open - 1.0).abs() < 1e-6);
        assert!((closed - AO_MIN).abs() < 1e-6);
        let half = vertex_occlusion(Vec3::ZERO, Vec3::Y, &|_, d, _| d.x > 0.0);
        assert!((AO_MIN..=1.0).contains(&half));
    }

    #[test]
    fn baking_darkens_and_keeps_material() {
        let mut c = build_meshes(&map(vec![bx([0, 0, 0], [4, 4, 4])]));
        let before = c[0].vertices[0].color_material;
        bake_occlusion(&mut c, |_, _, _| true);
        let after = c[0].vertices[0].color_material;
        assert!(after[0] < before[0] && after[3] == before[3]);
    }
}
