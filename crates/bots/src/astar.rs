//! A* over the navigation graph.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::graph::{Edge, EdgeKind, NavGraph, NodeId};

/// Cost of an edge for the search: its travel time, never less than the time
/// to cover its straight length at `vmax`. This keeps the heuristic
/// `|p_n - p_goal| / vmax` admissible and consistent for any edge, including
/// teleporters whose travel time is shorter than their length allows.
pub(crate) fn edge_cost(graph: &NavGraph, from: NodeId, e: &Edge, vmax: f32) -> f32 {
    let len = graph.nodes[from as usize]
        .pos
        .distance(graph.nodes[e.to as usize].pos);
    e.time.max(len / vmax)
}

/// Shortest-time chain of `(node, kind of the edge reaching it)` from `start`
/// (excluded) to `goal` (included), with its cost. Ties are broken by node
/// index, so the result is deterministic.
pub(crate) fn astar(
    graph: &NavGraph,
    start: NodeId,
    goal: NodeId,
    vmax: f32,
) -> Option<(Vec<(NodeId, EdgeKind)>, f32)> {
    let vmax = vmax.max(f32::EPSILON);
    let n = graph.nodes.len();
    let goal_pos = graph.nodes[goal as usize].pos;
    let h = |id: NodeId| graph.nodes[id as usize].pos.distance(goal_pos) / vmax;
    let mut g = vec![f32::INFINITY; n];
    let mut came: Vec<Option<(NodeId, EdgeKind)>> = vec![None; n];
    let mut closed = vec![false; n];
    // Non-negative floats order like their bit patterns.
    let mut open: BinaryHeap<Reverse<(u32, NodeId)>> = BinaryHeap::new();
    g[start as usize] = 0.0;
    open.push(Reverse((h(start).to_bits(), start)));
    while let Some(Reverse((_, id))) = open.pop() {
        if closed[id as usize] {
            continue;
        }
        closed[id as usize] = true;
        if id == goal {
            let mut chain = Vec::new();
            let mut cur = goal;
            while let Some((prev, kind)) = came[cur as usize] {
                chain.push((cur, kind));
                cur = prev;
            }
            chain.reverse();
            return Some((chain, g[goal as usize]));
        }
        for e in graph.edges_from(id) {
            let cost = g[id as usize] + edge_cost(graph, id, e, vmax);
            if cost < g[e.to as usize] {
                g[e.to as usize] = cost;
                came[e.to as usize] = Some((id, e.kind));
                closed[e.to as usize] = false;
                open.push(Reverse(((cost + h(e.to)).to_bits(), e.to)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use faset_core::glam::Vec3;

    use super::*;
    use crate::graph::Node;

    fn edge(to: NodeId, time: f32) -> Edge {
        Edge {
            to,
            kind: EdgeKind::Walk,
            time,
        }
    }

    /// Nodes on a line, 10 m apart; the direct edge 0 -> 3 is slower than the
    /// detour 0 -> 1 -> 2 -> 3, and a shortcut through node 4 beats both.
    fn hand_built() -> NavGraph {
        let nodes = [
            (0.0, 0.0),
            (10.0, 0.0),
            (20.0, 0.0),
            (30.0, 0.0),
            (15.0, 5.0),
        ]
        .iter()
        .map(|&(x, z)| Node::at(Vec3::new(x, 0.0, z)))
        .collect();
        let edges = vec![
            (0, edge(3, 9.0)),
            (0, edge(1, 1.5)),
            (1, edge(2, 1.5)),
            (2, edge(3, 1.5)),
            (0, edge(4, 2.0)),
            (4, edge(3, 2.0)),
            (3, edge(0, 0.1)),
        ];
        NavGraph::from_parts(nodes, edges)
    }

    #[test]
    fn returns_the_shortest_time_path() {
        let g = hand_built();
        let (chain, cost) = astar(&g, 0, 3, 15.0).unwrap();
        let ids: Vec<_> = chain.iter().map(|c| c.0).collect();
        assert_eq!(ids, vec![4, 3]);
        assert!((cost - 4.0).abs() < 1e-5, "{cost}");
        // Without the shortcut, the detour (4.5 s) beats the direct edge (9 s).
        let nodes = g.nodes().to_vec();
        let edges = vec![
            (0, edge(3, 9.0)),
            (0, edge(1, 1.5)),
            (1, edge(2, 1.5)),
            (2, edge(3, 1.5)),
        ];
        let g2 = NavGraph::from_parts(nodes, edges);
        let (chain, cost) = astar(&g2, 0, 3, 15.0).unwrap();
        assert_eq!(chain.iter().map(|c| c.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!((cost - 4.5).abs() < 1e-5);
    }

    #[test]
    fn stays_optimal_whatever_vmax() {
        // A teleporter-like edge: 0.1 s for 30 m. With vmax 15 its cost is
        // raised to 2 s, with vmax 1000 it stays 0.1 s; both are optimal.
        let nodes = (0..3)
            .map(|i| Node::at(Vec3::new(i as f32 * 15.0, 0.0, 0.0)))
            .collect();
        let edges = vec![(0, edge(2, 0.1)), (0, edge(1, 2.5)), (1, edge(2, 2.5))];
        let g = NavGraph::from_parts(nodes, edges);
        for vmax in [15.0, 1000.0] {
            let (chain, cost) = astar(&g, 0, 2, vmax).unwrap();
            assert_eq!(chain.len(), 1);
            assert!(
                (cost - 0.1f32.max(30.0 / vmax)).abs() < 1e-5,
                "{vmax}: {cost}"
            );
        }
    }

    #[test]
    fn unreachable_goal_gives_none() {
        let g = hand_built();
        assert!(astar(&g, 3, 3, 15.0).is_some());
        let nodes = vec![Node::at(Vec3::ZERO), Node::at(Vec3::X)];
        let g = NavGraph::from_parts(nodes, vec![]);
        assert!(astar(&g, 0, 1, 15.0).is_none());
    }

    #[test]
    fn find_path_returns_waypoints_from_start_to_goal() {
        let g = hand_built();
        let p = g
            .find_path(Vec3::new(0.0, 0.0, 0.0), Vec3::new(30.0, 0.0, 0.0), 15.0)
            .unwrap();
        assert_eq!(p.waypoints.first().unwrap().pos, Vec3::ZERO);
        assert_eq!(p.waypoints.last().unwrap().pos, Vec3::new(30.0, 0.0, 0.0));
        assert!((p.time - 4.0).abs() < 1e-5);
    }
}
