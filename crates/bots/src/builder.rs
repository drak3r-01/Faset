//! Graph construction: floor sampling, walk links, then controller
//! simulations for drop, jump, pad and teleporter edges. See the crate docs
//! for the decisions.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use faset_core::glam::Vec3;
use faset_core::DT;
use faset_mapfmt::ZoneKind;
use faset_sim::controller::{
    sin_cos_q, step_player_in, MoveCmd, MoveParams, PlayerState, BUTTON_JUMP,
};
use faset_sim::map_collision::{PLAYER_HEIGHT, PLAYER_RADIUS};
use faset_sim::zones::{ZoneEvent, ZoneTracker};
use faset_sim::{StaticWorld, Zones};

use crate::graph::{cell_of, centre_offset, Edge, EdgeKind, Grid, NavGraph, Node, NodeId};
use crate::CELL_SIZE;

/// Gap between the capsule and the floor for the "fits standing" test.
const FIT_LIFT: f32 = 0.005;
/// Gap between the capsule and the floor for sweeps between nodes.
const SWEEP_LIFT: f32 = 0.02;
/// Rest gap of the capsule at the start of a simulation.
const START_LIFT: f32 = 0.001;
/// Tolerance on the step height, metres (0.45 is linked, 0.46 is not).
const STEP_TOL: f32 = 1e-3;
/// Walk phase before a ledge test gives up, ticks.
const WALK_TICKS: u32 = 24;
/// Longest flight, ticks (4 s).
const FLIGHT_TICKS: u32 = 240;
/// Fall below the lowest volume before a simulation is dropped, metres.
const VOID_MARGIN: f32 = 8.0;
/// Highest difference between a landing height and a node height, metres.
const LAND_TOL_Y: f32 = 0.4;

/// Extra walk time per unit of grade on terrain links (decision, lot 6.10):
/// a 45 degree slope takes twice as long as flat ground.
const SLOPE_COST: f32 = 1.0;

/// Horizontal distance between two points.
fn horizontal(a: Vec3, b: Vec3) -> f32 {
    ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt()
}

/// A floor hit of one column.
struct Floor {
    y: f32,
    normal: Vec3,
    volume: u32,
}

/// Read-only data shared by all build phases.
struct Ctx<'a> {
    world: &'a StaticWorld,
    zones: &'a Zones,
    params: &'a MoveParams,
    nodes: Vec<Node>,
    grid: Grid,
    void_y: f32,
    /// Nodes of each trigger zone.
    trigger_nodes: Vec<Vec<NodeId>>,
    /// Walk edges as CSR, for the "already linked" test of the simulations.
    walk_start: Vec<u32>,
    walk_edges: Vec<Edge>,
}

/// The eight horizontal directions: cell offset, unit vector and yaw.
fn directions() -> [([i32; 2], Vec3, u16); 8] {
    std::array::from_fn(|k| {
        let q = (k as u16) * 8192;
        let (s, c) = sin_cos_q(q);
        let sgn = |v: f32| {
            if v > 0.3 {
                1
            } else if v < -0.3 {
                -1
            } else {
                0
            }
        };
        ([sgn(-s), sgn(-c)], Vec3::new(-s, 0.0, -c), q)
    })
}

/// Runs `f` on consecutive chunks of `0..n` in threads and concatenates the
/// results in chunk order, so the output does not depend on the thread count.
fn par_ranges<T: Send>(n: usize, f: impl Fn(Range<usize>) -> Vec<T> + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let chunk = n.div_ceil(threads * 16).max(1);
    let chunks = n.div_ceil(chunk);
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Vec<T>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads.min(chunks) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= chunks {
                    break;
                }
                let out = f(i * chunk..((i + 1) * chunk).min(n));
                done.lock().expect("no panic while locked").push((i, out));
            });
        }
    });
    let mut done = done.into_inner().expect("no panic while locked");
    done.sort_by_key(|d| d.0);
    done.into_iter().flat_map(|d| d.1).collect()
}

/// Sorts directed edges by `(from, to, kind, time)`, drops duplicates (the
/// fastest of equal `(from, to, kind)` stays) and lays them out per node.
pub(crate) fn csr(n: usize, mut edges: Vec<(NodeId, Edge)>) -> (Vec<u32>, Vec<Edge>) {
    edges.sort_by(|a, b| {
        (a.0, a.1.to, a.1.kind)
            .cmp(&(b.0, b.1.to, b.1.kind))
            .then(a.1.time.total_cmp(&b.1.time))
    });
    edges.dedup_by_key(|e| (e.0, e.1.to, e.1.kind));
    let mut start = vec![0u32; n + 1];
    for (from, _) in &edges {
        start[*from as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    (start, edges.into_iter().map(|e| e.1).collect())
}

pub(crate) fn build(
    world: StaticWorld,
    zones: &Zones,
    spawns: Vec<Vec3>,
    params: &MoveParams,
) -> NavGraph {
    let started = std::time::Instant::now();
    let min_ny = params.min_ground_normal_y();
    let (nodes, grid, void_y) = sample_nodes(&world, zones, min_ny);
    let mut trigger_nodes = vec![Vec::new(); zones.volumes().len()];
    for (i, n) in nodes.iter().enumerate() {
        if let Some(z) = n.trigger {
            trigger_nodes[z].push(i as NodeId);
        }
    }
    let mut ctx = Ctx {
        world: &world,
        zones,
        params,
        nodes,
        grid,
        void_y,
        trigger_nodes,
        walk_start: Vec::new(),
        walk_edges: Vec::new(),
    };
    let walk = ctx.walk_links();
    let mask = ctx.boundary_masks(&walk);
    (ctx.walk_start, ctx.walk_edges) = csr(ctx.nodes.len(), walk.clone());
    let mut all = walk;
    let sims = par_ranges(ctx.nodes.len(), |r| {
        r.flat_map(|a| ctx.simulate_node(a as NodeId, mask[a]))
            .collect()
    });
    all.extend(sims);
    let (edge_start, edges) = csr(ctx.nodes.len(), all);
    tracing::debug!(
        nodes = ctx.nodes.len(),
        edges = edges.len(),
        ms = started.elapsed().as_millis() as u64,
        "navigation graph built"
    );
    let Ctx { nodes, grid, .. } = ctx;
    NavGraph {
        nodes,
        edge_start,
        edges,
        grid: Some(grid),
        world,
        spawns,
        min_ny,
    }
}

/// Samples every cell column of the map; see the crate docs.
fn sample_nodes(world: &StaticWorld, zones: &Zones, min_ny: f32) -> (Vec<Node>, Grid, f32) {
    let boxes: Vec<_> = world
        .volumes()
        .iter()
        .map(|v| v.shape.compute_aabb(&v.pose))
        .collect();
    if boxes.is_empty() {
        let grid = Grid {
            origin: [0, 0],
            dims: [0, 0],
            col_start: vec![0],
        };
        return (Vec::new(), grid, 0.0);
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for b in &boxes {
        lo = lo.min(Vec3::new(b.mins.x, b.mins.y, b.mins.z));
        hi = hi.max(Vec3::new(b.maxs.x, b.maxs.y, b.maxs.z));
    }
    let vol_min_y: Vec<f32> = boxes.iter().map(|b| b.mins.y).collect();
    let origin = [cell_of(lo.x), cell_of(lo.z)];
    let dims = [
        (cell_of(hi.x) - origin[0] + 1) as usize,
        (cell_of(hi.z) - origin[1] + 1) as usize,
    ];
    let (top, bottom) = (hi.y + 1.0, lo.y - 1.0);
    let columns = par_ranges(dims[0] * dims[1], |range| {
        range
            .map(|col| {
                let c = [
                    origin[0] + (col / dims[1]) as i32,
                    origin[1] + (col % dims[1]) as i32,
                ];
                sample_column(world, &vol_min_y, c, top, bottom, min_ny)
            })
            .collect()
    });
    let mut nodes = Vec::new();
    let mut col_start = Vec::with_capacity(columns.len() + 1);
    for (col, floors) in columns.into_iter().enumerate() {
        col_start.push(nodes.len() as u32);
        let c = [
            origin[0] + (col / dims[1]) as i32,
            origin[1] + (col % dims[1]) as i32,
        ];
        for f in floors {
            let pos = Vec3::new(
                (c[0] as f32 + 0.5) * CELL_SIZE,
                f.y,
                (c[1] as f32 + 0.5) * CELL_SIZE,
            );
            let mut trigger = None;
            let mut kill = false;
            let (zlo, zhi) = (
                pos + Vec3::new(-PLAYER_RADIUS, 0.0, -PLAYER_RADIUS),
                pos + Vec3::new(PLAYER_RADIUS, PLAYER_HEIGHT, PLAYER_RADIUS),
            );
            for (i, z) in zones.volumes().iter().enumerate() {
                let inside = zlo.cmplt(z.max).all() && zhi.cmpgt(z.min).all();
                if !inside {
                    continue;
                }
                match z.kind {
                    ZoneKind::KillVolume => kill = true,
                    ZoneKind::JumpPad { .. } | ZoneKind::Teleporter { .. } => {
                        trigger = trigger.or(Some(i));
                    }
                    _ => {}
                }
            }
            if !kill {
                nodes.push(Node {
                    pos,
                    normal: f.normal,
                    volume: f.volume,
                    cell: c,
                    trigger,
                });
            }
        }
    }
    col_start.push(nodes.len() as u32);
    let grid = Grid {
        origin,
        dims,
        col_start,
    };
    (nodes, grid, lo.y - VOID_MARGIN)
}

/// Walkable floors of one cell column, lowest first.
fn sample_column(
    world: &StaticWorld,
    vol_min_y: &[f32],
    c: [i32; 2],
    top: f32,
    bottom: f32,
    min_ny: f32,
) -> Vec<Floor> {
    let x = (c[0] as f32 + 0.5) * CELL_SIZE;
    let z = (c[1] as f32 + 0.5) * CELL_SIZE;
    let mut out = Vec::new();
    let mut y0 = top;
    while y0 > bottom {
        let Some(hit) = world.ray_cast(Vec3::new(x, y0, z), Vec3::NEG_Y, y0 - bottom) else {
            break;
        };
        let y = y0 - hit.distance;
        // A zero distance means the ray started inside a volume: not a floor.
        if hit.distance > 1e-4 && hit.normal.y >= min_ny {
            let centre = Vec3::new(x, y + centre_offset(hit.normal.y, FIT_LIFT), z);
            if !world.intersects(centre, PLAYER_HEIGHT) {
                out.push(Floor {
                    y,
                    normal: hit.normal,
                    volume: hit.volume as u32,
                });
            }
        }
        // The vertical line stays inside a convex volume down to its bottom
        // (its bounds are used: exact for boxes, ramps and prisms).
        y0 = vol_min_y[hit.volume].min(y) - 1e-3;
    }
    out.reverse();
    out
}

impl Ctx<'_> {
    fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    /// Walk edges between neighbouring cells, both directions. Nodes inside a
    /// pad or teleporter have no walk edge out (entering the zone launches).
    fn walk_links(&self) -> Vec<(NodeId, Edge)> {
        let mut offsets = Vec::new();
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                let ring = dx.abs().max(dz.abs());
                if ring > 0 && (dx > 0 || (dx == 0 && dz > 0)) {
                    offsets.push(([dx, dz], ring));
                }
            }
        }
        par_ranges(self.nodes.len(), |range| {
            let mut out = Vec::new();
            for a in range {
                let na = &self.nodes[a];
                for &(off, ring) in &offsets {
                    let cell = [na.cell[0] + off[0], na.cell[1] + off[1]];
                    for b in self.grid.nodes_in(cell) {
                        let dy = self.node(b).pos.y - na.pos.y;
                        if ring == 2 && dy.abs() <= 0.02 {
                            continue;
                        }
                        if !self.walk_link(a as NodeId, b) {
                            continue;
                        }
                        let time = na.pos.distance(self.node(b).pos) / self.params.run_speed
                            * self.slope_factor(a as NodeId, b);
                        let edge = |to| Edge {
                            to,
                            kind: EdgeKind::Walk,
                            time,
                        };
                        if na.trigger.is_none() {
                            out.push((a as NodeId, edge(b)));
                        }
                        if self.node(b).trigger.is_none() {
                            out.push((b, edge(a as NodeId)));
                        }
                    }
                }
            }
            out
        })
    }

    /// Whether a bot walks between two nodes: a step of at most the step
    /// height, or the same plane (a slope), with the capsule clear all along
    /// when swept 2 cm above the higher floor (the step-up).
    fn walk_link(&self, a: NodeId, b: NodeId) -> bool {
        let (na, nb) = (self.node(a), self.node(b));
        let dy = nb.pos.y - na.pos.y;
        let step = dy.abs() <= self.params.max_step + STEP_TOL;
        if !step && self.on_terrain(na, nb) {
            // Terrain: one continuous surface, walkable while the slope
            // between the two nodes stays within the controller's limit.
            if dy.abs() > horizontal(na.pos, nb.pos) * self.max_grade() + STEP_TOL {
                return false;
            }
        } else if !step {
            let n = na.normal;
            let plane_y =
                na.pos.y - (n.x * (nb.pos.x - na.pos.x) + n.z * (nb.pos.z - na.pos.z)) / n.y;
            let same_plane = na.volume == nb.volume
                && na.normal.dot(nb.normal) > 0.9999
                && (plane_y - nb.pos.y).abs() < 0.01;
            if !same_plane {
                return false;
            }
        }
        let high = if na.pos.y >= nb.pos.y { na } else { nb };
        let cy = high.pos.y + centre_offset(high.normal.y, SWEEP_LIFT);
        let from = Vec3::new(na.pos.x, cy, na.pos.z);
        let to = Vec3::new(nb.pos.x, cy, nb.pos.z);
        !self.world.intersects(from, PLAYER_HEIGHT)
            && !self.world.intersects(to, PLAYER_HEIGHT)
            && self
                .world
                .shape_cast(from, to - from, PLAYER_HEIGHT)
                .is_none()
    }

    /// Whether both nodes stand on terrain triangles.
    fn on_terrain(&self, na: &Node, nb: &Node) -> bool {
        self.world.is_terrain(na.volume as usize) && self.world.is_terrain(nb.volume as usize)
    }

    /// Steepest walkable grade (rise over run) of the controller.
    fn max_grade(&self) -> f32 {
        let ny = self.params.min_ground_normal_y().max(1e-3);
        (1.0 - ny * ny).max(0.0).sqrt() / ny
    }

    /// Walk time multiplier of a terrain link: `1 + SLOPE_COST * grade`, so
    /// bots prefer to walk around hills (1 elsewhere).
    fn slope_factor(&self, a: NodeId, b: NodeId) -> f32 {
        let (na, nb) = (self.node(a), self.node(b));
        if !self.on_terrain(na, nb) {
            return 1.0;
        }
        let run = horizontal(na.pos, nb.pos).max(1e-3);
        1.0 + SLOPE_COST * (nb.pos.y - na.pos.y).abs() / run
    }

    /// Per node, a bit per direction (index of [`directions`]) that has a
    /// walk edge to the neighbouring cell.
    fn boundary_masks(&self, walk: &[(NodeId, Edge)]) -> Vec<u8> {
        let dirs = directions();
        let mut mask = vec![0u8; self.nodes.len()];
        for (from, e) in walk {
            let (a, b) = (self.node(*from), self.node(e.to));
            let off = [b.cell[0] - a.cell[0], b.cell[1] - a.cell[1]];
            if let Some(k) = dirs.iter().position(|d| d.0 == off) {
                mask[*from as usize] |= 1 << k;
            }
        }
        mask
    }

    fn walk_linked(&self, a: NodeId, b: NodeId) -> bool {
        let (s, e) = (self.walk_start[a as usize], self.walk_start[a as usize + 1]);
        self.walk_edges[s as usize..e as usize]
            .iter()
            .any(|x| x.to == b)
    }

    /// A node of the cell containing `feet`, at about its height.
    fn landing_node(&self, feet: Vec3) -> Option<NodeId> {
        let cell = [cell_of(feet.x), cell_of(feet.z)];
        self.grid
            .nodes_in(cell)
            .map(|id| (id, (self.node(id).pos.y - feet.y).abs()))
            .filter(|&(_, dy)| dy <= LAND_TOL_Y)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    fn start_state(&self, a: NodeId, vel: Vec3) -> PlayerState {
        let n = self.node(a);
        let mut s = PlayerState::spawn(n.pos + Vec3::Y * centre_offset(n.normal.y, START_LIFT));
        s.on_ground = true;
        s.vel = vel;
        s
    }

    /// Out-edges found by simulating the controller from node `a`.
    fn simulate_node(&self, a: NodeId, walked: u8) -> Vec<(NodeId, Edge)> {
        let mut out = Vec::new();
        let mut record = |kind: EdgeKind, to: NodeId, ticks: u32| {
            if to != a && !self.walk_linked(a, to) {
                out.push((
                    a,
                    Edge {
                        to,
                        kind,
                        time: ticks as f32 * DT,
                    },
                ));
            }
        };
        let node = self.node(a);
        if node.trigger.is_some() {
            if let Some((kind, to, ticks)) = self.simulate_trigger(a) {
                record(kind, to, ticks);
            }
            return out;
        }
        for (k, (_, dir, yaw)) in directions().into_iter().enumerate() {
            if walked & (1 << k) != 0 {
                continue;
            }
            let walk = MoveCmd {
                yaw,
                move_y: 127,
                ..MoveCmd::default()
            };
            let jump = MoveCmd {
                buttons: BUTTON_JUMP,
                ..walk
            };
            let vel = dir * self.params.run_speed;
            // Jump at once.
            let mut r = Runner::new(self, self.start_state(a, vel));
            if let Some(b) = r.run(jump, walk) {
                record(r.ground_edge(), b, r.ticks);
            }
            // Walk on until the ground is lost, then jump (coyote time) or
            // keep walking (drop).
            let mut w = Runner::new(self, self.start_state(a, vel));
            let mut lost = false;
            while w.ticks < WALK_TICKS {
                // The walk phase cannot land: `was_air` ends it first.
                if !matches!(w.tick(walk), Step::Go) {
                    break;
                }
                if w.was_air {
                    lost = true;
                    break;
                }
            }
            if lost {
                let mut j = w.clone();
                if let Some(b) = j.run(jump, walk) {
                    record(j.ground_edge(), b, j.ticks);
                }
                if let Some(b) = w.run(walk, walk) {
                    record(w.ground_edge(), b, w.ticks);
                }
            }
        }
        out
    }

    /// The effect of the pad or teleporter a node lies in: kind, landing node
    /// and ticks.
    fn simulate_trigger(&self, a: NodeId) -> Option<(EdgeKind, NodeId, u32)> {
        let mut r = Runner::new(self, self.start_state(a, Vec3::ZERO));
        let mut events = Vec::new();
        self.zones
            .apply(&mut r.state, &mut r.tracker, self.params, &mut events);
        let kind = events.iter().find_map(|e| match e {
            ZoneEvent::Triggered(ZoneKind::JumpPad { .. }) => Some(EdgeKind::Pad),
            ZoneEvent::Triggered(ZoneKind::Teleporter { .. }) => Some(EdgeKind::Teleport),
            _ => None,
        })?;
        // The zone put the bot in the air, even if it lands on the first tick.
        r.was_air = true;
        let cmd = MoveCmd::default();
        let to = r.run(cmd, cmd)?;
        Some((kind, to, r.ticks))
    }
}

/// Outcome of one simulated tick.
enum Step {
    Go,
    Landed(NodeId),
    Dead,
}

/// One simulated bot: controller state, zone membership and counters.
#[derive(Clone)]
struct Runner<'a> {
    ctx: &'a Ctx<'a>,
    state: PlayerState,
    tracker: ZoneTracker,
    events: Vec<ZoneEvent>,
    ticks: u32,
    was_air: bool,
    jumped: bool,
}

impl<'a> Runner<'a> {
    fn new(ctx: &'a Ctx<'a>, state: PlayerState) -> Self {
        Self {
            ctx,
            state,
            tracker: ZoneTracker::default(),
            events: Vec::new(),
            ticks: 0,
            was_air: false,
            jumped: false,
        }
    }

    /// Kind of an edge that ended on the ground: a jump if one fired, else a
    /// drop.
    fn ground_edge(&self) -> EdgeKind {
        if self.jumped {
            EdgeKind::Jump
        } else {
            EdgeKind::Drop
        }
    }

    /// Ticks with `first`, then `next`, until a landing (`Some`) or death.
    fn run(&mut self, first: MoveCmd, next: MoveCmd) -> Option<NodeId> {
        let mut cmd = first;
        loop {
            match self.tick(cmd) {
                Step::Go => cmd = next,
                Step::Landed(b) => return Some(b),
                Step::Dead => return None,
            }
        }
    }

    fn tick(&mut self, cmd: MoveCmd) -> Step {
        let ctx = self.ctx;
        let ev = step_player_in(&mut self.state, cmd, ctx.world, ctx.params, DT);
        self.jumped |= ev.jumped;
        self.ticks += 1;
        let feet = self.state.feet();
        self.events.clear();
        ctx.zones.apply(
            &mut self.state,
            &mut self.tracker,
            ctx.params,
            &mut self.events,
        );
        let mut zone = 0;
        for e in &self.events {
            match e {
                ZoneEvent::Entered(i) => zone = *i,
                ZoneEvent::Triggered(ZoneKind::KillVolume) => return Step::Dead,
                // Landing in the zone of another pad or teleporter.
                ZoneEvent::Triggered(ZoneKind::JumpPad { .. } | ZoneKind::Teleporter { .. }) => {
                    return ctx.trigger_nodes[zone]
                        .iter()
                        .map(|&id| (id, ctx.node(id).pos.distance_squared(feet)))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map_or(Step::Dead, |(id, _)| Step::Landed(id));
                }
                _ => {}
            }
        }
        if self.state.on_ground {
            if self.was_air {
                return ctx
                    .landing_node(self.state.feet())
                    .map_or(Step::Dead, Step::Landed);
            }
        } else {
            self.was_air = true;
            if self.state.pos.y < ctx.void_y {
                return Step::Dead;
            }
        }
        if self.ticks >= FLIGHT_TICKS || (!self.was_air && self.ticks > WALK_TICKS) {
            return Step::Dead;
        }
        Step::Go
    }
}
