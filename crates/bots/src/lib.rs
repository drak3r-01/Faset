//! Bot navigation (plan lot 4.9): a navigation graph generated from a map,
//! A* pathfinding and path smoothing.
//!
//! * [`NavGraph::build`] samples the map in cells of 0.5 m, links walkable
//!   cells (walk, drop, jump, jump pad, teleporter edges) using the real
//!   character controller of `faset-sim`, and is pure and deterministic
//!   (no GPU, no clock, no hash tables; threads only split independent work
//!   that is merged in a fixed order).
//! * [`NavGraph::find_path`] runs A* with a travel-time cost and smooths the
//!   result; [`Replanner`] re-plans every 0.5 s and when the goal changes.
//! * [`NavGraph::reachable_from`] and [`NavGraph::spawn_connectivity`] serve
//!   the map validator.
//!
//! Units: metre, second, radian; -Z forward, Y up. Positions of nodes and
//! waypoints are feet positions.
//!
//! Decisions (unspecified by the plan lot):
//! - Cells sit on the 0.5 m grid of the map (cell `i` spans `[0.5 i, 0.5 i + 0.5)`);
//!   a node is the centre of a cell column on one floor, so a column holds one
//!   node per walkable floor.
//! - A cell is walkable when the standing capsule (0.35 m, 1.8 m) fits 5 mm
//!   above a floor hit by a downward ray whose normal has `n_y` at least the
//!   controller threshold (`MoveParams::min_ground_normal_y`, 45 degrees).
//!   Cells within the capsule radius of a wall are therefore not nodes, so a
//!   step can be 0.75 m from the nearest node of the lower floor: cells up to
//!   two columns apart are linked when their floors differ.
//! - Walk edges: step at most `MoveParams::max_step` (+1 mm), or the same
//!   plane (same volume, same normal) for slopes. The clearance check sweeps
//!   the capsule 2 cm above the higher floor, which models the step-up.
//! - Jump and drop edges start from boundary nodes (nodes with an unlinked
//!   neighbour among the 8 directions) and run the controller at run speed
//!   toward that direction: a drop walks on, a jump fires at tick 0 or just
//!   after the ground is lost (coyote time). Run-up distance is assumed.
//! - A pad or teleporter zone makes the nodes inside it trigger nodes whose
//!   only out-edge is the zone effect, simulated with `Zones::apply`; nodes in
//!   a kill volume are dropped.
//! - Edge cost is the simulated travel time; walking costs `distance / v_run`.
//!   For the heuristic to stay admissible whatever the edge, the cost used by
//!   A* is `max(time, distance / vmax)`: see [`DEFAULT_VMAX`].
//! - Smoothing only merges consecutive walk segments.
#![forbid(unsafe_code)]

mod astar;
mod builder;
mod graph;
mod replan;

pub use graph::{
    Edge, EdgeKind, NavGraph, Node, NodeId, Path, ReachableSet, SpawnConnectivity, Waypoint,
    DEFAULT_VMAX,
};
pub use replan::{Replanner, REPLAN_INTERVAL};

/// Side of a navigation cell, metres.
pub const CELL_SIZE: f32 = 0.5;
