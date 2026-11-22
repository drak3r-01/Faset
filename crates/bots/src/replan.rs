//! Periodic re-planning.

use faset_core::glam::Vec3;

use crate::graph::{NavGraph, Path};

/// Seconds between two re-plans of an unchanged goal.
pub const REPLAN_INTERVAL: f32 = 0.5;

/// Goals closer than this are the same goal, metres.
const SAME_GOAL: f32 = 1e-3;

/// Keeps the current path of one bot and re-plans every
/// [`REPLAN_INTERVAL`] seconds and whenever the goal changes.
#[derive(Debug, Clone)]
pub struct Replanner {
    vmax: f32,
    since_plan: f32,
    goal: Option<Vec3>,
    path: Option<Path>,
}

impl Replanner {
    /// A re-planner using `vmax` for the search (see
    /// [`crate::DEFAULT_VMAX`]).
    #[must_use]
    pub fn new(vmax: f32) -> Self {
        Self {
            vmax,
            since_plan: 0.0,
            goal: None,
            path: None,
        }
    }

    /// Advances by `dt` seconds. Plans again when there is no plan yet, the
    /// goal moved, or 0.5 s passed; returns whether it did.
    pub fn update(&mut self, graph: &NavGraph, from: Vec3, goal: Vec3, dt: f32) -> bool {
        self.since_plan += dt;
        let moved = self
            .goal
            .is_none_or(|g| g.distance_squared(goal) > SAME_GOAL * SAME_GOAL);
        // Small tolerance so that 30 ticks of 1/60 s count as 0.5 s.
        if !moved && self.since_plan < REPLAN_INTERVAL - 1e-4 {
            return false;
        }
        self.goal = Some(goal);
        self.since_plan = 0.0;
        self.path = graph.find_path(from, goal, self.vmax);
        true
    }

    /// The latest path, `None` before the first plan or when the goal was
    /// unreachable.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_ref()
    }

    /// Forgets the plan and the goal; the next update plans again.
    pub fn clear(&mut self) {
        self.goal = None;
        self.path = None;
        self.since_plan = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, EdgeKind, Node};

    #[test]
    fn replans_on_goal_change_and_every_half_second() {
        let nodes = vec![Node::at(Vec3::ZERO), Node::at(Vec3::new(5.0, 0.0, 0.0))];
        let edges = vec![(
            0,
            Edge {
                to: 1,
                kind: EdgeKind::Walk,
                time: 1.0,
            },
        )];
        let g = NavGraph::from_parts(nodes, edges);
        let mut r = Replanner::new(15.0);
        let goal = Vec3::new(5.0, 0.0, 0.0);
        assert!(r.update(&g, Vec3::ZERO, goal, 0.0), "first plan");
        assert!(r.path().is_some());
        let dt = 1.0 / 60.0;
        let mut plans = 0;
        for _ in 0..60 {
            plans += usize::from(r.update(&g, Vec3::ZERO, goal, dt));
        }
        assert_eq!(plans, 2, "one plan every 0.5 s over one second");
        assert!(r.update(&g, Vec3::ZERO, Vec3::ZERO, dt), "goal changed");
    }
}
