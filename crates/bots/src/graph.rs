//! Graph types, queries and path smoothing.

use faset_core::glam::Vec3;
use faset_mapfmt::MapFile;
use faset_sim::controller::MoveParams;
use faset_sim::map_collision::{PLAYER_HEIGHT, PLAYER_RADIUS};
use faset_sim::{StaticWorld, Zones};

use crate::{astar, builder, CELL_SIZE};

/// Index of a node in [`NavGraph::nodes`].
pub type NodeId = u32;

/// Default `vmax` for [`NavGraph::find_path`], metres per second.
///
/// Decision: 15 m/s. The fastest edges the builder produces are the walk
/// (7 m/s) and jump pad launches (a pad velocity is capped at the controller
/// `max_speed`, 30 m/s, but real pads are about 12 to 13 m/s). The search never
/// relies on it for correctness: the cost of an edge is
/// `max(time, length / vmax)`, so `h(n) = |p_n - p_goal| / vmax` is admissible
/// and consistent for any `vmax`; a larger value only makes the search wider.
pub const DEFAULT_VMAX: f32 = 15.0;

/// How an edge is travelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// Walk, step up or down at most the step height, or follow a slope.
    Walk,
    /// Walk off a ledge and fall to a lower floor.
    Drop,
    /// Jump (simulated with the controller).
    Jump,
    /// Launched by a jump pad zone.
    Pad,
    /// Moved by a teleporter zone.
    Teleport,
}

/// A directed edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub to: NodeId,
    pub kind: EdgeKind,
    /// Travel time, seconds.
    pub time: f32,
}

/// A walkable cell on one floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    /// Feet position: centre of the cell, height of the floor.
    pub pos: Vec3,
    /// Floor normal.
    pub normal: Vec3,
    /// Index of the floor's volume in the collision world.
    pub volume: u32,
    /// Cell column `[x, z]` (cell `i` spans `[0.5 i, 0.5 i + 0.5)`).
    pub cell: [i32; 2],
    /// Pad or teleporter zone (index in the map's zones) the node lies in.
    pub trigger: Option<usize>,
}

impl Node {
    /// A flat-floor node at `pos` (hand-built graphs and tests).
    #[must_use]
    pub fn at(pos: Vec3) -> Self {
        Self {
            pos,
            normal: Vec3::Y,
            volume: 0,
            cell: [cell_of(pos.x), cell_of(pos.z)],
            trigger: None,
        }
    }
}

pub(crate) fn cell_of(v: f32) -> i32 {
    (v / CELL_SIZE).floor() as i32
}

/// Dense grid of cell columns; nodes are stored column by column, each column
/// sorted by ascending height.
#[derive(Debug, Clone)]
pub(crate) struct Grid {
    pub origin: [i32; 2],
    pub dims: [usize; 2],
    /// `dims[0] * dims[1] + 1` offsets into the node array.
    pub col_start: Vec<u32>,
}

impl Grid {
    pub fn column(&self, c: [i32; 2]) -> Option<usize> {
        let ix = usize::try_from(c[0] - self.origin[0]).ok()?;
        let iz = usize::try_from(c[1] - self.origin[1]).ok()?;
        (ix < self.dims[0] && iz < self.dims[1]).then_some(ix * self.dims[1] + iz)
    }

    /// Node ids of a column (empty outside the grid).
    pub fn nodes_in(&self, c: [i32; 2]) -> std::ops::Range<u32> {
        self.column(c)
            .map_or(0..0, |i| self.col_start[i]..self.col_start[i + 1])
    }
}

/// One waypoint of a [`Path`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waypoint {
    /// Feet position, metres.
    pub pos: Vec3,
    /// How this waypoint is reached from the previous one (`Walk` for the
    /// first waypoint).
    pub kind: EdgeKind,
}

/// A path from a start to a goal.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub waypoints: Vec<Waypoint>,
    /// Travel time along the graph, seconds.
    pub time: f32,
}

/// Nodes reachable from a point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableSet {
    pub(crate) reached: Vec<bool>,
    pub(crate) count: usize,
}

impl ReachableSet {
    #[must_use]
    pub fn contains(&self, node: NodeId) -> bool {
        self.reached.get(node as usize).copied().unwrap_or(false)
    }

    /// Number of reachable nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Reachable nodes over all nodes of the graph, in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.reached.is_empty() {
            0.0
        } else {
            self.count as f32 / self.reached.len() as f32
        }
    }
}

/// Result of [`NavGraph::spawn_connectivity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnConnectivity {
    /// Number of spawn points of the map.
    pub spawns: usize,
    /// Spawns with no node near them.
    pub unplaced: Vec<usize>,
    /// `(from, to)` spawn pairs where `to` cannot be reached from `from`.
    pub unreachable: Vec<(usize, usize)>,
}

impl SpawnConnectivity {
    /// Every spawn has a node and every spawn reaches every other.
    #[must_use]
    pub fn all_connected(&self) -> bool {
        self.unplaced.is_empty() && self.unreachable.is_empty()
    }
}

/// The navigation graph of a map.
#[derive(Clone)]
pub struct NavGraph {
    pub(crate) nodes: Vec<Node>,
    /// `nodes.len() + 1` offsets into `edges`.
    pub(crate) edge_start: Vec<u32>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) grid: Option<Grid>,
    pub(crate) world: StaticWorld,
    pub(crate) spawns: Vec<Vec3>,
    pub(crate) min_ny: f32,
}

impl std::fmt::Debug for NavGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavGraph")
            .field("nodes", &self.nodes.len())
            .field("edges", &self.edges.len())
            .finish_non_exhaustive()
    }
}

/// Capsule centre height above the floor for a capsule resting on a plane of
/// normal height `ny`, plus `lift` metres.
pub(crate) fn centre_offset(ny: f32, lift: f32) -> f32 {
    let half = PLAYER_HEIGHT * 0.5 - PLAYER_RADIUS;
    half + PLAYER_RADIUS / ny.max(0.1) + lift
}

impl NavGraph {
    /// Builds the graph of a map with the controller `params`. Pure and
    /// deterministic. The map is expanded here (`faset_mapfmt::expand` is
    /// idempotent), so a parsed or an expanded map may be passed.
    #[must_use]
    pub fn build(map: &MapFile, params: &MoveParams) -> Self {
        let map = faset_mapfmt::expand(map);
        let world = faset_sim::MapCollision::build(&map).world().clone();
        let spawns = map.spawns.iter().map(|s| Vec3::from_array(s.at)).collect();
        Self::build_world(world, &Zones::from_map(&map), spawns, params)
    }

    /// Builds the graph of a bare collision world with its zones and spawns.
    #[must_use]
    pub fn build_world(
        world: StaticWorld,
        zones: &Zones,
        spawns: Vec<Vec3>,
        params: &MoveParams,
    ) -> Self {
        builder::build(world, zones, spawns, params)
    }

    /// A graph from explicit nodes and directed edges `(from, edge)`, without
    /// collision (hand-built graphs and tests). Nearest-node queries scan all
    /// nodes and smoothing keeps every waypoint.
    #[must_use]
    pub fn from_parts(nodes: Vec<Node>, edges: Vec<(NodeId, Edge)>) -> Self {
        let (edge_start, edges) = builder::csr(nodes.len(), edges);
        Self {
            nodes,
            edge_start,
            edges,
            grid: None,
            world: StaticWorld::default(),
            spawns: Vec::new(),
            min_ny: MoveParams::default().min_ground_normal_y(),
        }
    }

    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Out-edges of a node, sorted by target, then kind.
    #[must_use]
    pub fn edges_from(&self, id: NodeId) -> &[Edge] {
        let i = id as usize;
        &self.edges[self.edge_start[i] as usize..self.edge_start[i + 1] as usize]
    }

    /// Spawn points of the map (feet positions).
    #[must_use]
    pub fn spawns(&self) -> &[Vec3] {
        &self.spawns
    }

    /// The node closest to a feet position, looking at the 7 x 7 cells around
    /// it; nodes more than 1.2 m above `p` are ignored (not stood on).
    #[must_use]
    pub fn nearest_node(&self, p: Vec3) -> Option<NodeId> {
        let mut best: Option<(f32, NodeId)> = None;
        let mut consider = |id: NodeId| {
            let n = &self.nodes[id as usize];
            if n.pos.y > p.y + 1.2 {
                return;
            }
            let d = n.pos.distance_squared(p);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        };
        if let Some(grid) = &self.grid {
            let (cx, cz) = (cell_of(p.x), cell_of(p.z));
            for dx in -3..=3 {
                for dz in -3..=3 {
                    grid.nodes_in([cx + dx, cz + dz]).for_each(&mut consider);
                }
            }
        } else {
            (0..self.nodes.len() as u32).for_each(&mut consider);
        }
        best.map(|(_, id)| id)
    }

    /// A* from the node nearest `from` to the node nearest `to` (feet
    /// positions). The cost of an edge is its travel time, raised to
    /// `length / vmax` when that is larger, and the heuristic is
    /// `|p_n - p_goal| / vmax`; see [`DEFAULT_VMAX`]. Consecutive walk
    /// waypoints are smoothed. `None` when a point has no node or the goal
    /// cannot be reached.
    #[must_use]
    pub fn find_path(&self, from: Vec3, to: Vec3, vmax: f32) -> Option<Path> {
        let start = self.nearest_node(from)?;
        let goal = self.nearest_node(to)?;
        let (chain, time) = astar::astar(self, start, goal, vmax)?;
        let mut raw = Vec::with_capacity(chain.len() + 2);
        raw.push(Waypoint {
            pos: from,
            kind: EdgeKind::Walk,
        });
        raw.extend(chain.into_iter().map(|(id, kind)| Waypoint {
            pos: self.nodes[id as usize].pos,
            kind,
        }));
        raw.push(Waypoint {
            pos: to,
            kind: EdgeKind::Walk,
        });
        let waypoints = smooth(&raw, |a, b| self.line_walkable(a, b));
        Some(Path { waypoints, time })
    }

    /// Nodes reachable from the node nearest `p` (empty without one).
    #[must_use]
    pub fn reachable_from(&self, p: Vec3) -> ReachableSet {
        match self.nearest_node(p) {
            Some(start) => self.reachable_from_node(start),
            None => ReachableSet {
                reached: vec![false; self.nodes.len()],
                count: 0,
            },
        }
    }

    /// Nodes reachable from `start`.
    #[must_use]
    pub fn reachable_from_node(&self, start: NodeId) -> ReachableSet {
        let mut reached = vec![false; self.nodes.len()];
        let mut stack = vec![start];
        reached[start as usize] = true;
        let mut count = 1;
        while let Some(n) = stack.pop() {
            for e in self.edges_from(n) {
                if !reached[e.to as usize] {
                    reached[e.to as usize] = true;
                    count += 1;
                    stack.push(e.to);
                }
            }
        }
        ReachableSet { reached, count }
    }

    /// Whether every spawn point of the map reaches every other one.
    #[must_use]
    pub fn spawn_connectivity(&self) -> SpawnConnectivity {
        let placed: Vec<Option<NodeId>> =
            self.spawns.iter().map(|&s| self.nearest_node(s)).collect();
        let unplaced = (0..placed.len()).filter(|&i| placed[i].is_none()).collect();
        let mut unreachable = Vec::new();
        for (i, from) in placed.iter().enumerate() {
            let Some(from) = *from else { continue };
            let set = self.reachable_from_node(from);
            for (j, to) in placed.iter().enumerate() {
                if i != j && to.is_some_and(|t| !set.contains(t)) {
                    unreachable.push((i, j));
                }
            }
        }
        SpawnConnectivity {
            spawns: self.spawns.len(),
            unplaced,
            unreachable,
        }
    }

    /// Whether a bot can walk the straight line between two feet positions:
    /// every sample (0.25 m apart) has a walkable floor within 10 cm of the
    /// line, the standing capsule fits there, and the capsule sweeps freely
    /// from sample to sample. Always `false` on a graph without collision.
    #[must_use]
    pub fn line_walkable(&self, a: Vec3, b: Vec3) -> bool {
        if self.world.volumes().is_empty() {
            return false;
        }
        const SAMPLE: f32 = 0.25;
        const LIFT: f32 = 0.02;
        let len = a.distance(b);
        let n = ((len / SAMPLE).ceil() as usize).max(1);
        let mut prev: Option<Vec3> = None;
        for i in 0..=n {
            let p = a.lerp(b, i as f32 / n as f32);
            let Some(hit) = self
                .world
                .ray_cast(p + Vec3::Y * 0.5, Vec3::NEG_Y, 1.0)
                .filter(|h| h.normal.y >= self.min_ny)
            else {
                return false;
            };
            let floor_y = p.y + 0.5 - hit.distance;
            if (floor_y - p.y).abs() > 0.1 {
                return false;
            }
            let centre = Vec3::new(p.x, floor_y + centre_offset(hit.normal.y, LIFT), p.z);
            if self.world.intersects(centre, PLAYER_HEIGHT) {
                return false;
            }
            if let Some(from) = prev {
                if self
                    .world
                    .shape_cast(from, centre - from, PLAYER_HEIGHT)
                    .is_some()
                {
                    return false;
                }
            }
            prev = Some(centre);
        }
        true
    }
}

/// Removes intermediate waypoints of runs of consecutive walk segments while
/// `walkable(a, b)` holds for the surrounding points; other segments (drop,
/// jump, pad, teleport) and their end points are kept.
pub(crate) fn smooth(raw: &[Waypoint], walkable: impl Fn(Vec3, Vec3) -> bool) -> Vec<Waypoint> {
    let mut out = vec![raw[0]];
    let mut i = 0;
    while i + 1 < raw.len() {
        if raw[i + 1].kind != EdgeKind::Walk {
            out.push(raw[i + 1]);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut k = i + 2;
        while k < raw.len() && raw[k].kind == EdgeKind::Walk {
            if walkable(raw[i].pos, raw[k].pos) {
                j = k;
            }
            k += 1;
        }
        out.push(raw[j]);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(x: f32, z: f32, kind: EdgeKind) -> Waypoint {
        Waypoint {
            pos: Vec3::new(x, 0.0, z),
            kind,
        }
    }

    #[test]
    fn smoothing_removes_collinear_walk_waypoints() {
        let raw: Vec<_> = (0..6).map(|i| wp(i as f32, 0.0, EdgeKind::Walk)).collect();
        let out = smooth(&raw, |_, _| true);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pos.x, 0.0);
        assert_eq!(out[1].pos.x, 5.0);
    }

    #[test]
    fn smoothing_keeps_corners_and_non_walk_segments() {
        // An L shape: the straight line across the corner is blocked.
        let blocked = |a: Vec3, b: Vec3| (a.x - b.x).abs() < 1e-6 || (a.z - b.z).abs() < 1e-6;
        let raw = vec![
            wp(0.0, 0.0, EdgeKind::Walk),
            wp(1.0, 0.0, EdgeKind::Walk),
            wp(2.0, 0.0, EdgeKind::Walk),
            wp(2.0, 1.0, EdgeKind::Walk),
            wp(2.0, 2.0, EdgeKind::Jump),
            wp(2.0, 3.0, EdgeKind::Walk),
            wp(2.0, 4.0, EdgeKind::Walk),
        ];
        let out = smooth(&raw, blocked);
        let pts: Vec<_> = out.iter().map(|w| (w.pos.x, w.pos.z, w.kind)).collect();
        assert_eq!(
            pts,
            vec![
                (0.0, 0.0, EdgeKind::Walk),
                (2.0, 0.0, EdgeKind::Walk),
                (2.0, 1.0, EdgeKind::Walk),
                (2.0, 2.0, EdgeKind::Jump),
                (2.0, 4.0, EdgeKind::Walk),
            ]
        );
    }
}
