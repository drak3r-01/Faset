//! Symmetry and prefab expansion.
//!
//! Every placement transform is exact on the integer grid: an optional
//! mirror across x = 0, then quarter turns about +Y, then a grid offset.

use crate::schema::{
    Axis, Brush, Dir, GridPos, Light, MapFile, Pickup, Pos, Prop, Spawn, Symmetry, Zone, ZoneKind,
    GRID_UNIT_M,
};

/// Rigid grid transform: `p -> R^turns * M^mirror_x * p + offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Xform {
    mirror_x: bool,
    /// Quarter turns counter-clockwise seen from above (0..4).
    turns: u8,
    /// Offset in half-metres.
    offset: GridPos,
}

impl Xform {
    fn symmetry(s: Symmetry) -> Option<Self> {
        let (mirror_x, turns) = match s {
            Symmetry::None => return None,
            Symmetry::MirrorX => (true, 0),
            // Mirror x then half turn: (x, z) -> (-x, z) -> (x, -z).
            Symmetry::MirrorZ => (true, 2),
            Symmetry::Rotate180 => (false, 2),
        };
        Some(Self {
            mirror_x,
            turns,
            offset: [0; 3],
        })
    }

    fn is_mirror(self) -> bool {
        self.mirror_x
    }

    /// Linear part applied to an `(x, z)` pair of any numeric type.
    fn linear<T: Copy + std::ops::Neg<Output = T>>(self, mut x: T, mut z: T) -> (T, T) {
        if self.mirror_x {
            x = -x;
        }
        for _ in 0..self.turns % 4 {
            // +90 degrees about +Y: (x, z) -> (z, -x).
            (x, z) = (z, -x);
        }
        (x, z)
    }

    fn grid(self, p: GridPos) -> GridPos {
        let (x, z) = self.linear(p[0], p[2]);
        [
            x + self.offset[0],
            p[1] + self.offset[1],
            z + self.offset[2],
        ]
    }

    #[allow(clippy::cast_precision_loss)]
    fn pos(self, p: Pos) -> Pos {
        let (x, z) = self.linear(p[0], p[2]);
        let o = self.offset.map(|v| v as f32 * GRID_UNIT_M);
        // `+ 0.0` turns a negated zero into a positive one.
        [x + o[0] + 0.0, p[1] + o[1] + 0.0, z + o[2] + 0.0]
    }

    /// Direction vector, no offset.
    fn vector(self, v: Pos) -> Pos {
        let (x, z) = self.linear(v[0], v[2]);
        [x + 0.0, v[1], z + 0.0]
    }

    /// Yaw in degrees (0 faces -Z, counter-clockwise seen from above).
    fn yaw(self, yaw: f32) -> f32 {
        let y = if self.mirror_x { -yaw } else { yaw };
        (y + 90.0 * f32::from(self.turns % 4)).rem_euclid(360.0) + 0.0
    }

    fn dir(self, d: Dir) -> Dir {
        let (x, z) = match d {
            Dir::PosX => (1, 0),
            Dir::NegX => (-1, 0),
            Dir::PosZ => (0, 1),
            Dir::NegZ => (0, -1),
        };
        match self.linear(x, z) {
            (1, _) => Dir::PosX,
            (-1, _) => Dir::NegX,
            (_, 1) => Dir::PosZ,
            _ => Dir::NegZ,
        }
    }

    fn axis(self, a: Axis) -> Axis {
        match (a, self.turns % 2 == 1) {
            (Axis::X, true) => Axis::Z,
            (Axis::Z, true) => Axis::X,
            (a, _) => a,
        }
    }

    /// Transforms both corners of a box and re-sorts them per axis.
    fn bounds(self, min: GridPos, max: GridPos) -> (GridPos, GridPos) {
        let a = self.grid(min);
        let b = self.grid(max);
        (
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        )
    }

    fn brush(self, b: &Brush) -> Brush {
        match b {
            Brush::Box { min, max, material } => {
                let (min, max) = self.bounds(*min, *max);
                Brush::Box {
                    min,
                    max,
                    material: material.clone(),
                }
            }
            Brush::Wedge {
                min,
                max,
                rise,
                material,
            } => {
                let (min, max) = self.bounds(*min, *max);
                Brush::Wedge {
                    min,
                    max,
                    rise: self.dir(*rise),
                    material: material.clone(),
                }
            }
            Brush::Cylinder {
                min,
                max,
                axis,
                sides,
                material,
            } => {
                let (min, max) = self.bounds(*min, *max);
                Brush::Cylinder {
                    min,
                    max,
                    axis: self.axis(*axis),
                    sides: *sides,
                    material: material.clone(),
                }
            }
            Brush::Prism {
                points,
                y_min,
                y_max,
                material,
            } => {
                let mut points: Vec<[i32; 2]> = points
                    .iter()
                    .map(|p| {
                        let g = self.grid([p[0], 0, p[1]]);
                        [g[0], g[2]]
                    })
                    .collect();
                // A mirror flips the winding; restore counter-clockwise.
                if self.is_mirror() {
                    points.reverse();
                }
                Brush::Prism {
                    points,
                    y_min: y_min + self.offset[1],
                    y_max: y_max + self.offset[1],
                    material: material.clone(),
                }
            }
            Brush::Stairs {
                min,
                max,
                rise,
                steps,
                material,
            } => {
                let (min, max) = self.bounds(*min, *max);
                Brush::Stairs {
                    min,
                    max,
                    rise: self.dir(*rise),
                    steps: *steps,
                    material: material.clone(),
                }
            }
        }
    }

    fn prop(self, p: &Prop) -> Prop {
        Prop {
            recipe: p.recipe.clone(),
            at: self.pos(p.at),
            yaw: self.yaw(p.yaw),
            scale: p.scale,
        }
    }

    fn spawn(self, s: &Spawn) -> Spawn {
        Spawn {
            at: self.pos(s.at),
            yaw: self.yaw(s.yaw),
            team: s.team.other(),
        }
    }

    fn pickup(self, p: &Pickup) -> Pickup {
        Pickup {
            kind: p.kind.clone(),
            at: self.pos(p.at),
            respawn_s: p.respawn_s,
        }
    }

    fn zone(self, z: &Zone) -> Zone {
        match z {
            Zone::Box { min, max, kind } => {
                let (min, max) = self.bounds(*min, *max);
                let kind = match kind {
                    ZoneKind::JumpPad { velocity } => ZoneKind::JumpPad {
                        velocity: self.vector(*velocity),
                    },
                    ZoneKind::Teleporter { target, yaw } => ZoneKind::Teleporter {
                        target: self.pos(*target),
                        yaw: self.yaw(*yaw),
                    },
                    other => other.clone(),
                };
                Zone::Box { min, max, kind }
            }
        }
    }

    fn light(self, l: &Light) -> Light {
        Light {
            at: self.pos(l.at),
            ..l.clone()
        }
    }
}

/// Replaces every prefab by its placed brushes and props, appended after the
/// map's own brushes and props in prefab then placement order. The result has
/// no prefabs.
#[must_use]
pub fn expand_prefabs(map: &MapFile) -> MapFile {
    let mut out = map.clone();
    out.prefabs.clear();
    for prefab in &map.prefabs {
        for place in &prefab.place {
            let t = Xform {
                mirror_x: false,
                turns: place.turns % 4,
                offset: place.at,
            };
            out.brushes
                .extend(prefab.brushes.iter().map(|b| t.brush(b)));
            out.props.extend(prefab.props.iter().map(|p| t.prop(p)));
        }
    }
    out
}

/// Adds the symmetric half declared by `symmetry` (prefabs are expanded
/// first). Brushes, props, spawns, pickups, zones and lights are all copied;
/// a copied spawn belongs to the other team. The result declares
/// [`Symmetry::None`]; a map without symmetry is returned unchanged (with
/// prefabs expanded).
#[must_use]
pub fn expand_symmetry(map: &MapFile) -> MapFile {
    let mut out = expand_prefabs(map);
    let Some(t) = Xform::symmetry(map.symmetry) else {
        return out;
    };
    let brushes: Vec<_> = out.brushes.iter().map(|b| t.brush(b)).collect();
    let props: Vec<_> = out.props.iter().map(|p| t.prop(p)).collect();
    let spawns: Vec<_> = out.spawns.iter().map(|s| t.spawn(s)).collect();
    let pickups: Vec<_> = out.pickups.iter().map(|p| t.pickup(p)).collect();
    let zones: Vec<_> = out.zones.iter().map(|z| t.zone(z)).collect();
    let lights: Vec<_> = out.lights.iter().map(|l| t.light(l)).collect();
    out.brushes.extend(brushes);
    out.props.extend(props);
    out.spawns.extend(spawns);
    out.pickups.extend(pickups);
    out.zones.extend(zones);
    out.lights.extend(lights);
    out.symmetry = Symmetry::None;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(mirror_x: bool, turns: u8) -> Xform {
        Xform {
            mirror_x,
            turns,
            offset: [0; 3],
        }
    }

    #[test]
    fn symmetry_planes() {
        let mx = Xform::symmetry(Symmetry::MirrorX).unwrap();
        let mz = Xform::symmetry(Symmetry::MirrorZ).unwrap();
        let r = Xform::symmetry(Symmetry::Rotate180).unwrap();
        assert_eq!(mx.grid([1, 2, 3]), [-1, 2, 3]);
        assert_eq!(mz.grid([1, 2, 3]), [1, 2, -3]);
        assert_eq!(r.grid([1, 2, 3]), [-1, 2, -3]);
        assert_eq!(mx.pos([0.0, 1.0, 2.0]), [0.0, 1.0, 2.0]);
        assert!(mx.pos([0.0, 1.0, 2.0])[0].is_sign_positive());
    }

    #[test]
    fn yaw_follows_forward_vector() {
        // Forward of yaw y (degrees) is (-sin y, -cos y) in (x, z).
        let fwd = |y: f32| {
            let r = y.to_radians();
            (-r.sin(), -r.cos())
        };
        for xf in [t(true, 0), t(true, 2), t(false, 1), t(false, 2), t(true, 3)] {
            for yaw in [0.0_f32, 15.0, 45.0, 90.0, 200.0] {
                let (fx, fz) = fwd(yaw);
                let (ex, ez) = xf.linear(fx, fz);
                let (gx, gz) = fwd(xf.yaw(yaw));
                assert!(
                    (ex - gx).abs() < 1e-5 && (ez - gz).abs() < 1e-5,
                    "{xf:?} {yaw}"
                );
            }
        }
        assert_eq!(t(true, 0).yaw(45.0), 315.0);
        assert_eq!(t(true, 2).yaw(45.0), 135.0);
        assert_eq!(t(false, 2).yaw(45.0), 225.0);
    }

    #[test]
    fn dirs_and_axes_rotate() {
        let q = t(false, 1);
        assert_eq!(q.dir(Dir::PosX), Dir::NegZ);
        assert_eq!(q.dir(Dir::NegZ), Dir::NegX);
        assert_eq!(t(true, 0).dir(Dir::PosX), Dir::NegX);
        assert_eq!(t(true, 0).dir(Dir::PosZ), Dir::PosZ);
        assert_eq!(q.axis(Axis::X), Axis::Z);
        assert_eq!(t(false, 2).axis(Axis::X), Axis::X);
    }

    #[test]
    fn mirrored_box_swaps_min_max() {
        let b = Brush::Box {
            min: [4, 0, -6],
            max: [10, 3, -3],
            material: "m".into(),
        };
        assert_eq!(
            t(true, 0).brush(&b),
            Brush::Box {
                min: [-10, 0, -6],
                max: [-4, 3, -3],
                material: "m".into(),
            }
        );
    }

    #[test]
    fn mirrored_prism_keeps_winding() {
        let b = Brush::Prism {
            points: vec![[0, 0], [2, 0], [0, -2]],
            y_min: 0,
            y_max: 2,
            material: "m".into(),
        };
        let area = |p: &[[i32; 2]]| -> i32 {
            (0..p.len())
                .map(|i| {
                    let (a, b) = (p[i], p[(i + 1) % p.len()]);
                    a[0] * b[1] - b[0] * a[1]
                })
                .sum()
        };
        let Brush::Prism { points: before, .. } = &b else {
            unreachable!()
        };
        let Brush::Prism { points: after, .. } = t(true, 0).brush(&b) else {
            unreachable!()
        };
        assert_eq!(area(before).signum(), area(&after).signum());
    }
}
