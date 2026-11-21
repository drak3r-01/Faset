//! Remote entity interpolation: other players are shown a deliberate delay
//! in the past, so two snapshots almost always bracket the render time.
//!
//! # Render time
//!
//! ```text
//! t_rendu = t_serveur - d        d = max(2I, I + 2j)
//! ```
//!
//! `I` is the snapshot interval ([`InterpolationConfig::snapshot_interval`])
//! and `j` the jitter measured by [`crate::ClockSync`].
//!
//! Decision: `t_serveur` is the server time as seen through the snapshot
//! stream, i.e. the tick of the snapshot that would arrive now. It is
//! estimated by filtering `S - local/Δt` over the received snapshots (the
//! same 0.9 / 0.1 filter as the clock), which averages out the arrival
//! jitter. A snapshot `2j` late is still covered because `d >= I + 2j`.
//! Every time is in ticks of the snapshot labels (the world tick after the
//! step a snapshot describes), the same labels as the server's hitbox
//! history, so the [`ViewTime`] a shooter saw can be rewound to directly.
//!
//! # Sampling
//!
//! * Position: cubic Hermite between the two bracketing snapshots, with
//!   their positions and velocities; `I` in the formula is the time between
//!   those two snapshots (decision: a lost snapshot widens the bracket, and
//!   the velocities must be scaled by the real gap).
//! * Angles: linear along the shortest path.
//! * Past the newest snapshot: linear extrapolation along its velocity for
//!   at most [`MAX_EXTRAPOLATION`] seconds, then the entity freezes there;
//!   angles are held (decision).
//! * Before the oldest snapshot: the oldest state (decision).

use std::collections::{BTreeMap, VecDeque};
use std::f32::consts::{PI, TAU};

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};

/// Longest extrapolation past the newest snapshot, in seconds.
pub const MAX_EXTRAPOLATION: f64 = 0.25;
/// Default snapshot interval `I` in seconds (the server's default 30 Hz).
pub const DEFAULT_SNAPSHOT_INTERVAL: f64 = 1.0 / 30.0;
/// Snapshots kept per entity at most (decision: about one second at 30 Hz).
pub const BUFFER_SNAPSHOTS: usize = 32;

/// Weight of a new sample in the stream clock filter.
const FILTER: f64 = 0.1;

fn dt() -> f64 {
    f64::from(DT)
}

/// The delay `d = max(2I, I + 2j)` in seconds, from the snapshot interval
/// and the jitter, both in seconds.
#[must_use]
pub fn interpolation_delay(interval: f64, jitter: f64) -> f64 {
    (2.0 * interval).max(interval + 2.0 * jitter)
}

/// Cubic Hermite interpolation between `p0` (velocity `v0`) and `p1`
/// (velocity `v1`), `interval` seconds apart, at fraction `s` in [0, 1]:
///
/// ```text
/// p(s) = (2s³ - 3s² + 1) p0 + (s³ - 2s² + s) I v0 + (-2s³ + 3s²) p1 + (s³ - s²) I v1
/// ```
#[must_use]
pub fn hermite(p0: Vec3, v0: Vec3, p1: Vec3, v1: Vec3, interval: f32, s: f32) -> Vec3 {
    let s2 = s * s;
    let s3 = s2 * s;
    p0 * (2.0 * s3 - 3.0 * s2 + 1.0)
        + v0 * ((s3 - 2.0 * s2 + s) * interval)
        + p1 * (-2.0 * s3 + 3.0 * s2)
        + v1 * ((s3 - s2) * interval)
}

/// Interpolates from angle `a` to angle `b` (radians) along the shortest
/// path; the result is not wrapped.
#[must_use]
pub fn lerp_angle(a: f32, b: f32, s: f32) -> f32 {
    let d = (b - a + PI).rem_euclid(TAU) - PI;
    a + d * s
}

/// What one snapshot says about a remote entity.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RemoteState {
    /// Capsule centre, metres.
    pub position: Vec3,
    /// Metres per second.
    pub velocity: Vec3,
    /// Radians.
    pub yaw: f32,
    /// Radians.
    pub pitch: f32,
}

/// How a [`RemoteView`] was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleMode {
    /// Between two snapshots (or clamped to the oldest one).
    Interpolated,
    /// Past the newest snapshot, within [`MAX_EXTRAPOLATION`].
    Extrapolated,
    /// Past the newest snapshot by more than [`MAX_EXTRAPOLATION`]: the
    /// entity stays where the extrapolation stopped.
    Frozen,
}

/// A remote entity as displayed at a render time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteView {
    /// Capsule centre, metres.
    pub position: Vec3,
    /// Radians.
    pub yaw: f32,
    /// Radians.
    pub pitch: f32,
    pub mode: SampleMode,
}

/// A render time as a tick and the fraction toward the next one: what a
/// fire command carries so the server can rewind to the shooter's view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTime {
    pub tick: Tick,
    /// In [0, 1).
    pub fraction: f32,
}

impl ViewTime {
    /// Splits a time in ticks (negative times give tick 0).
    #[must_use]
    pub fn from_ticks(ticks: f64) -> Self {
        let t = ticks.max(0.0);
        let tick = t.floor();
        Self {
            tick: Tick(tick as u32),
            fraction: ((t - tick) as f32).clamp(0.0, 1.0 - f32::EPSILON),
        }
    }

    /// The time in ticks.
    #[must_use]
    pub fn ticks(self) -> f64 {
        f64::from(self.tick.0) + f64::from(self.fraction)
    }
}

/// The snapshots received for one remote entity, oldest first.
#[derive(Debug, Clone, Default)]
pub struct InterpolationBuffer {
    samples: VecDeque<(Tick, RemoteState)>,
}

impl InterpolationBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the state of snapshot `tick`. Snapshots not newer than the
    /// newest one are ignored; returns whether it was kept. At most
    /// [`BUFFER_SNAPSHOTS`] are kept.
    pub fn push(&mut self, tick: Tick, state: RemoteState) -> bool {
        if self.samples.back().is_some_and(|&(t, _)| tick <= t) {
            return false;
        }
        self.samples.push_back((tick, state));
        while self.samples.len() > BUFFER_SNAPSHOTS {
            self.samples.pop_front();
        }
        true
    }

    /// Number of snapshots held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Tick of the newest snapshot.
    #[must_use]
    pub fn newest(&self) -> Option<Tick> {
        self.samples.back().map(|&(t, _)| t)
    }

    /// Drops the snapshots that can no longer bracket `render` (ticks):
    /// keeps the newest one at or before it and everything after.
    pub fn prune(&mut self, render: f64) {
        while self.samples.len() >= 2 && f64::from(self.samples[1].0 .0) <= render {
            self.samples.pop_front();
        }
    }

    /// The entity at `render` (ticks), or `None` without any snapshot.
    #[must_use]
    pub fn sample(&self, render: f64) -> Option<RemoteView> {
        let &(first_tick, first) = self.samples.front()?;
        let &(last_tick, last) = self.samples.back()?;
        let last_t = f64::from(last_tick.0);
        if render >= last_t {
            let ahead = (render - last_t) * dt();
            let (mode, run) = if ahead <= 0.0 {
                (SampleMode::Interpolated, 0.0)
            } else if ahead <= MAX_EXTRAPOLATION {
                (SampleMode::Extrapolated, ahead)
            } else {
                (SampleMode::Frozen, MAX_EXTRAPOLATION)
            };
            return Some(RemoteView {
                position: last.position + last.velocity * run as f32,
                yaw: last.yaw,
                pitch: last.pitch,
                mode,
            });
        }
        if render <= f64::from(first_tick.0) {
            return Some(RemoteView {
                position: first.position,
                yaw: first.yaw,
                pitch: first.pitch,
                mode: SampleMode::Interpolated,
            });
        }
        // First snapshot strictly after `render`; the one before brackets it.
        let i = self
            .samples
            .iter()
            .position(|&(t, _)| f64::from(t.0) > render)?;
        let (t0, a) = self.samples[i - 1];
        let (t1, b) = self.samples[i];
        let gap = f64::from(t1.0 - t0.0);
        let s = ((render - f64::from(t0.0)) / gap) as f32;
        let interval = (gap * dt()) as f32;
        Some(RemoteView {
            position: hermite(a.position, a.velocity, b.position, b.velocity, interval, s),
            yaw: lerp_angle(a.yaw, b.yaw, s),
            pitch: lerp_angle(a.pitch, b.pitch, s),
            mode: SampleMode::Interpolated,
        })
    }
}

/// Tuning of a [`RemoteInterpolation`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterpolationConfig {
    /// Snapshot interval `I` in seconds (1 / the server's snapshot rate).
    pub snapshot_interval: f64,
}

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            snapshot_interval: DEFAULT_SNAPSHOT_INTERVAL,
        }
    }
}

/// Every remote entity's buffer and the snapshot stream clock.
///
/// Feed it every decoded snapshot with [`on_snapshot`](Self::on_snapshot)
/// (the remote entities only), then each frame take
/// [`render_time`](Self::render_time) and [`sample`](Self::sample) the
/// entities at it.
#[derive(Debug, Clone, Default)]
pub struct RemoteInterpolation {
    config: InterpolationConfig,
    buffers: BTreeMap<NetId, InterpolationBuffer>,
    /// Filtered `S - local/Δt` in ticks.
    offset: Option<f64>,
    newest: Option<Tick>,
}

impl RemoteInterpolation {
    #[must_use]
    pub fn new(config: InterpolationConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn config(&self) -> InterpolationConfig {
        self.config
    }

    /// Takes snapshot `tick`, received at `local_time` (seconds on any
    /// steady local clock), with the remote entities it holds. Entities it
    /// does not list are dropped (decision: a snapshot lists every player).
    /// Duplicate and out-of-order snapshots are ignored; returns whether
    /// this one was used.
    pub fn on_snapshot(
        &mut self,
        tick: Tick,
        local_time: f64,
        entities: impl IntoIterator<Item = (NetId, RemoteState)>,
    ) -> bool {
        if self.newest.is_some_and(|n| tick <= n) {
            return false;
        }
        self.newest = Some(tick);
        let sample = f64::from(tick.0) - local_time / dt();
        self.offset = Some(match self.offset {
            None => sample,
            Some(o) => o + FILTER * (sample - o),
        });
        let mut seen = Vec::new();
        for (id, state) in entities {
            self.buffers.entry(id).or_default().push(tick, state);
            seen.push(id);
        }
        self.buffers.retain(|id, _| seen.contains(id));
        true
    }

    /// The delay `d` in seconds for the given jitter.
    #[must_use]
    pub fn delay(&self, jitter: f64) -> f64 {
        interpolation_delay(self.config.snapshot_interval, jitter)
    }

    /// Estimated stream time `t_serveur` in ticks at `local_time`.
    #[must_use]
    pub fn stream_time(&self, local_time: f64) -> Option<f64> {
        Some(local_time / dt() + self.offset?)
    }

    /// Render time `t_serveur - d` in ticks at `local_time`, with the
    /// jitter `j` in seconds; `None` before the first snapshot.
    #[must_use]
    pub fn render_time(&self, local_time: f64, jitter: f64) -> Option<f64> {
        Some(self.stream_time(local_time)? - self.delay(jitter) / dt())
    }

    /// The remote entity `id` at `render` (ticks).
    #[must_use]
    pub fn sample(&self, id: NetId, render: f64) -> Option<RemoteView> {
        self.buffers.get(&id)?.sample(render)
    }

    /// Every remote entity at `render`, ascending id.
    pub fn sample_all(&self, render: f64) -> impl Iterator<Item = (NetId, RemoteView)> + '_ {
        self.buffers
            .iter()
            .filter_map(move |(&id, b)| Some((id, b.sample(render)?)))
    }

    /// Drops snapshots older than needed for `render` (call once per frame).
    pub fn prune(&mut self, render: f64) {
        for b in self.buffers.values_mut() {
            b.prune(render);
        }
    }

    /// The buffer of one entity.
    #[must_use]
    pub fn buffer(&self, id: NetId) -> Option<&InterpolationBuffer> {
        self.buffers.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn state(x: f32, vx: f32, yaw: f32) -> RemoteState {
        RemoteState {
            position: Vec3::new(x, 1.0, 0.0),
            velocity: Vec3::new(vx, 0.0, 0.0),
            yaw,
            pitch: 0.0,
        }
    }

    #[test]
    fn delay_follows_the_formula() {
        let i = 1.0 / 30.0;
        assert!((interpolation_delay(i, 0.0) - 2.0 * i).abs() < 1e-12);
        assert!((interpolation_delay(i, 0.05) - (i + 0.1)).abs() < 1e-12);
    }

    #[test]
    fn constant_velocity_is_reproduced_exactly() {
        let mut b = InterpolationBuffer::new();
        // 6 m/s along X, snapshots every 2 ticks.
        for k in 0..5u32 {
            let t = 2 * k;
            b.push(Tick(t), state(6.0 * t as f32 * DT, 6.0, 0.0));
        }
        for i in 0..80 {
            let t = f64::from(i) * 0.1;
            let v = b.sample(t).unwrap();
            let want = 6.0 * t as f32 * DT;
            assert!(
                (v.position.x - want).abs() < 1e-5,
                "{t}: {} vs {want}",
                v.position.x
            );
            assert_eq!(v.mode, SampleMode::Interpolated);
        }
    }

    #[test]
    fn jump_arc_follows_the_velocities() {
        // A parabola sampled at 30 Hz: Hermite with the true velocities is
        // exact for a quadratic, where linear interpolation cuts the arc.
        let g = -9.81f32;
        let at = |t: f32| (8.0 * t + 0.5 * g * t * t, 8.0 + g * t);
        let i = 2.0 * DT;
        let (p0, v0) = at(0.0);
        let (p1, v1) = at(i);
        let mid = hermite(
            Vec3::new(0.0, p0, 0.0),
            Vec3::new(0.0, v0, 0.0),
            Vec3::new(0.0, p1, 0.0),
            Vec3::new(0.0, v1, 0.0),
            i,
            0.5,
        );
        assert!((mid.y - at(i / 2.0).0).abs() < 1e-5);
    }

    #[test]
    fn angles_take_the_shortest_path() {
        let a = 3.0f32;
        let b = -3.0f32;
        let m = lerp_angle(a, b, 0.5);
        // Halfway through pi, not through zero.
        assert!((m.rem_euclid(TAU) - PI).abs() < 1e-5, "{m}");
        assert!((lerp_angle(0.1, 0.3, 0.5) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn stale_snapshots_and_departed_entities() {
        let mut r = RemoteInterpolation::default();
        assert!(r.render_time(0.0, 0.0).is_none());
        assert!(r.on_snapshot(
            Tick(10),
            0.0,
            [
                (NetId(1), state(0.0, 0.0, 0.0)),
                (NetId(2), state(1.0, 0.0, 0.0))
            ]
        ));
        assert!(!r.on_snapshot(Tick(10), 0.01, [(NetId(1), state(5.0, 0.0, 0.0))]));
        assert!(!r.on_snapshot(Tick(8), 0.01, []));
        assert!(r.on_snapshot(
            Tick(12),
            2.0 * f64::from(DT),
            [(NetId(1), state(1.0, 0.0, 0.0))]
        ));
        assert!(r.buffer(NetId(2)).is_none());
        assert_eq!(r.buffer(NetId(1)).unwrap().len(), 2);
        // Stream time is the newest tick at its arrival; render d behind.
        let render = r.render_time(2.0 * f64::from(DT), 0.0).unwrap();
        assert!((render - (12.0 - 4.0)).abs() < 1e-6, "{render}");
        let v = ViewTime::from_ticks(12.25);
        assert_eq!(v.tick, Tick(12));
        assert!((v.ticks() - 12.25).abs() < 1e-6);
    }

    #[test]
    fn prune_keeps_the_bracket() {
        let mut b = InterpolationBuffer::new();
        for t in [0, 2, 4, 6] {
            b.push(Tick(t), state(t as f32, 0.0, 0.0));
        }
        b.prune(4.5);
        assert_eq!(b.len(), 2);
        assert!((b.sample(5.0).unwrap().position.x - 5.0).abs() < 0.5);
    }

    proptest! {
        #[test]
        fn hermite_meets_the_endpoints(
            p0 in (-100.0f32..100.0, -100.0f32..100.0, -100.0f32..100.0),
            v0 in (-20.0f32..20.0, -20.0f32..20.0, -20.0f32..20.0),
            p1 in (-100.0f32..100.0, -100.0f32..100.0, -100.0f32..100.0),
            v1 in (-20.0f32..20.0, -20.0f32..20.0, -20.0f32..20.0),
            interval in 0.01f32..0.2,
        ) {
            let p0 = Vec3::new(p0.0, p0.1, p0.2);
            let p1 = Vec3::new(p1.0, p1.1, p1.2);
            let v0 = Vec3::new(v0.0, v0.1, v0.2);
            let v1 = Vec3::new(v1.0, v1.1, v1.2);
            prop_assert_eq!(hermite(p0, v0, p1, v1, interval, 0.0), p0);
            prop_assert_eq!(hermite(p0, v0, p1, v1, interval, 1.0), p1);
        }

        #[test]
        fn buffer_meets_the_snapshots_and_extrapolation_stops(
            x0 in -50.0f32..50.0,
            x1 in -50.0f32..50.0,
            v in -20.0f32..20.0,
            gap in 1u32..8,
            past in 0.0f64..2.0,
        ) {
            let mut b = InterpolationBuffer::new();
            b.push(Tick(100), state(x0, v, 0.0));
            b.push(Tick(100 + gap), state(x1, v, 0.0));
            prop_assert_eq!(b.sample(100.0).unwrap().position.x, x0);
            let end = f64::from(100 + gap);
            prop_assert_eq!(b.sample(end).unwrap().position.x, x1);
            // Past the newest snapshot: moving for 0.25 s at most.
            let t = end + past / dt();
            let view = b.sample(t).unwrap();
            let run = past.min(MAX_EXTRAPOLATION) as f32;
            prop_assert!((view.position.x - (x1 + v * run)).abs() < 1e-3);
            let later = b.sample(t + 1.0 / dt()).unwrap();
            prop_assert_eq!(later.mode, SampleMode::Frozen);
            prop_assert!((later.position.x - (x1 + v * MAX_EXTRAPOLATION as f32)).abs() < 1e-3);
            if past > MAX_EXTRAPOLATION + 1e-6 {
                prop_assert_eq!(view.mode, SampleMode::Frozen);
                prop_assert!((view.position - later.position).length() < 1e-4);
            }
        }
    }
}
