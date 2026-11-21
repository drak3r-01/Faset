//! Walk graph over 0.5 m cells.
//!
//! Decision (lot 3.3): until lot 4.9 provides the `bots` graph, the validator
//! walks the map with its own search: 8 neighbours per cell, the capsule must
//! fit, each move climbs or descends at most `step_m` (0.45 m, which keeps
//! the slope under 45 degrees on 0.5 m cells). No jump, drop, jump-pad or
//! teleporter edges. Moves are symmetric, so one search from a spawn tells
//! whether every other point can reach it too.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use faset_core::glam::Vec3;
use faset_sim::map_collision::PLAYER_HEIGHT;
use faset_sim::MapCollision;

/// Cell size in metres (plan section 17).
pub const CELL_M: f32 = 0.5;
/// Height quantum separating floors stacked in one cell, in metres.
const LEVEL_M: f32 = 0.25;
/// Clearance between a lifted capsule and the floor it steps off, in metres.
const LIFT_MARGIN_M: f32 = 0.02;

/// A walkable place: a cell and the floor level in it.
type Key = (i32, i32, i32);

/// One reached place.
#[derive(Debug, Clone, Copy)]
pub struct Node {
    /// Feet position, cell centre in x and z.
    pub feet: Vec3,
    /// Walking distance from the nearest source, in metres.
    pub dist: f32,
}

/// Result of a search from a set of sources.
pub struct Reach {
    pub nodes: HashMap<Key, Node>,
    /// Cells with at least one blocked move (an obstacle beside them).
    pub bordered: usize,
}

#[allow(clippy::cast_possible_truncation)]
fn key_of(p: Vec3) -> Key {
    (
        (p.x / CELL_M).floor() as i32,
        (p.z / CELL_M).floor() as i32,
        (p.y / LEVEL_M).round() as i32,
    )
}

fn snap(p: Vec3) -> Vec3 {
    let centre = |v: f32| ((v / CELL_M).floor() + 0.5) * CELL_M;
    Vec3::new(centre(p.x), p.y, centre(p.z))
}

/// Centre of the standing capsule whose feet are at `feet`.
fn centre(feet: Vec3, lift: f32) -> Vec3 {
    feet + Vec3::Y * (PLAYER_HEIGHT * 0.5 + lift)
}

/// Outcome of one move.
enum Move {
    /// Walkable: the feet position reached.
    To(Vec3),
    /// No floor, too steep or no room.
    No,
    /// An obstacle stands in the way.
    Blocked,
}

fn step(col: &MapCollision, from: Vec3, dir: Vec3, len: f32, step_m: f32) -> Move {
    let lift = step_m + LIFT_MARGIN_M;
    let start = centre(from, lift);
    if col.overlaps_capsule(start, PLAYER_HEIGHT) {
        return Move::No;
    }
    let delta = dir * len;
    if col.sweep_capsule(start, delta, PLAYER_HEIGHT).is_some() {
        return Move::Blocked;
    }
    let end = start + delta;
    let drop = lift + step_m + LIFT_MARGIN_M;
    let Some(hit) = col.sweep_capsule(end, Vec3::NEG_Y * drop, PLAYER_HEIGHT) else {
        return Move::No;
    };
    let feet = Vec3::new(end.x, end.y - hit.toi * drop - PLAYER_HEIGHT * 0.5, end.z);
    let dy = feet.y - from.y;
    if dy.abs() > step_m + 1e-3 || col.overlaps_capsule(centre(feet, 0.01), PLAYER_HEIGHT) {
        return Move::No;
    }
    Move::To(feet)
}

const DIRS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (1.0, 1.0),
    (1.0, -1.0),
    (-1.0, 1.0),
    (-1.0, -1.0),
];

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn millimetres(m: f32) -> u32 {
    (m * 1000.0).round() as u32
}

/// Dijkstra over the walk graph from `sources` (feet positions).
#[must_use]
pub fn reach(col: &MapCollision, sources: &[Vec3], step_m: f32) -> Reach {
    let mut nodes: HashMap<Key, Node> = HashMap::new();
    let mut heap = BinaryHeap::new();
    let mut bordered = HashSet::new();
    for &s in sources {
        let snapped = snap(s);
        let feet = if col.overlaps_capsule(centre(snapped, 0.01), PLAYER_HEIGHT) {
            s
        } else {
            snapped
        };
        let key = key_of(feet);
        nodes.entry(key).or_insert(Node { feet, dist: 0.0 });
        heap.push(Reverse((0u32, key)));
    }
    while let Some(Reverse((cost, key))) = heap.pop() {
        let node = nodes[&key];
        if cost > millimetres(node.dist) {
            continue;
        }
        for (dx, dz) in DIRS {
            let v = Vec3::new(dx, 0.0, dz);
            let len = CELL_M * v.length();
            match step(col, node.feet, v.normalize(), len, step_m) {
                Move::Blocked => {
                    bordered.insert(key);
                }
                Move::No => {}
                Move::To(feet) => {
                    let k = key_of(feet);
                    let dist = node.dist + len;
                    if nodes.get(&k).is_none_or(|n| dist + 1e-4 < n.dist) {
                        nodes.insert(k, Node { feet, dist });
                        heap.push(Reverse((millimetres(dist), k)));
                    }
                }
            }
        }
    }
    Reach {
        nodes,
        bordered: bordered.len(),
    }
}

/// Horizontal distance within which a walker picks an item up, in metres.
pub const PICKUP_REACH_M: f32 = 1.0;
/// How far above a walkable floor an item may float, in metres.
pub const PICKUP_MAX_HEIGHT_M: f32 = 1.5;

impl Reach {
    /// Walking distance to the nearest node within reach of `at`, if any.
    #[must_use]
    pub fn distance_to(&self, at: Vec3) -> Option<f32> {
        self.nodes
            .values()
            .filter(|n| {
                let d = Vec3::new(n.feet.x - at.x, 0.0, n.feet.z - at.z).length();
                let up = at.y - n.feet.y;
                d <= PICKUP_REACH_M && (-0.25..=PICKUP_MAX_HEIGHT_M).contains(&up)
            })
            .map(|n| n.dist)
            .min_by(f32::total_cmp)
    }
}
