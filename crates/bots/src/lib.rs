//! Bots: navigation (plan lot 4.9) and decision, perception, aim and
//! difficulty levels (plan lot 4.10).
//!
//! # Navigation (4.9)
//!
//! A navigation graph generated from a map, A* pathfinding and path smoothing.
//!
//! # Brain (4.10)
//!
//! * [`BotBrain::think`] scores six actions each tick ([`utility`]), the best
//!   wins with a 15 % bonus for the current one, then follows the
//!   navigation path ([`BotView::nav`]) and aims ([`aim`]). It returns the
//!   same [`InputCommand`] a human client sends, so the server loop treats
//!   bots as virtual clients through the existing input path.
//! * [`Perception`] gives a 110 degree field of view, limited range, line of
//!   sight by raycast refreshed every 4 ticks, hearing of close shots, and a
//!   queue that delays every observation by the level's reaction time.
//! * [`aim`] holds the ballistic lead (smallest positive root of the quadratic),
//!   gravity compensation in two Newton iterations, the lob arc, the angular
//!   spring and the time-correlated error `e <- e exp(-dt/tau) + sigma sqrt(1 -
//!   exp(-2 dt/tau)) N(0,1)`.
//! * [`Level`] and [`LevelTable`]: four levels, data in `data/levels.ron`.
//! * [`Bot`] bundles a brain with its perception: what the server loop holds
//!   per bot. [`plan_bot_fill`] gives the number of bots wanted.
//!
//! Host contract: the host copies the world into a [`RawState`] each tick
//! (hostile living players, weapon numbers as data, pickups, the objective;
//! `bots` depends on `sim` and `protocol` only, not on `mode-fps` or
//! `minigame-api`) and applies the returned command like a client's.
//!
//! Decisions (unspecified by the plan lot 4.10):
//! - Field of view: 110 degrees full width (55 degrees each side).
//! - Line of sight: every 4 ticks per bot, for every enemy inside the view
//!   cone and range; the bot's phase spreads the refreshes.
//! - Utility constants live in [`UtilityConfig`] (heal `w` = 1.8, `k` = 12 m).
//! - Lob: the plus root of the plan's formula is the lob, the minus root the
//!   direct arc.
//! - The fire intent is bit 2 of `buttons` ([`BUTTON_FIRE`]); the protocol
//!   defines no fire bit.
//! - Plans are refreshed every 0.5 s or on a goal change (at most every
//!   0.25 s); the first plan of each bot waits a seed-derived 0 to 29 ticks
//!   so that 16 bots do not run A* on the same tick.
//!
//! # Navigation notes (4.9)
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

pub mod aim;
mod astar;
mod brain;
mod builder;
mod fill;
mod follow;
mod graph;
mod level;
pub mod perception;
mod replan;
pub mod utility;
mod view;

pub use brain::{Bot, BotBrain};
pub use faset_protocol::InputCommand;
pub use fill::plan_bot_fill;
pub use graph::{
    Edge, EdgeKind, NavGraph, Node, NodeId, Path, ReachableSet, SpawnConnectivity, Waypoint,
    DEFAULT_VMAX,
};
pub use level::{Level, LevelError, LevelParams, LevelTable, MoveFlags, DEFAULT_LEVELS_RON};
pub use perception::{Perception, PerceptionConfig, FOV_DEGREES, LOS_PERIOD};
pub use replan::{Replanner, REPLAN_INTERVAL};
pub use utility::{Action, UtilityConfig};
pub use view::{
    BotView, HeardShot, PickupInfo, PickupKind, RawEntity, RawState, SelfState, Sighting,
    WeaponParams, BUTTON_FIRE,
};

/// Side of a navigation cell, metres.
pub const CELL_SIZE: f32 = 0.5;
