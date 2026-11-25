//! Clock synchronization: round-trip estimate, lead over the server and
//! speed modulation.
//!
//! # Round-trip time and jitter
//!
//! Samples come from timestamped exchanges: the `renet` packet
//! acknowledgements of the connection (`ClientEndpoint::rtt`, one reading per
//! received snapshot). Each sample `s` (seconds) goes through the plan's
//! filter, the jitter using the deviation from the estimate before the
//! sample:
//!
//! ```text
//! j   <- 0.9 j   + 0.1 |s - rtt|
//! rtt <- 0.9 rtt + 0.1 s
//! ```
//!
//! The first sample initialises `rtt` directly (decision) and `j` to zero.
//!
//! # Lead (decision: the plan defines neither lead)
//!
//! Server tick `n` is processed at real time `n·Δt`; the snapshot of tick `S`
//! (the world tick after the step) leaves while tick `S - 1` is processed.
//! The client time `τ` is the last simulated tick plus the fraction of the
//! next one already accumulated, in ticks.
//!
//! * The *lead* of a command is how many ticks before the server consumes it
//!   it arrives.
//! * Measured lead: `a_measured = ō - rtt/Δt`, where `ō` is the filtered
//!   offset `τ - (S - 1)` sampled when each snapshot arrives (same 0.9 / 0.1
//!   filter, propagated between snapshots by the client's own speed change).
//!   `ō` is the client's advance on the server plus the down-link delay, so
//!   removing the full round trip leaves the advance minus the up-link
//!   delay, i.e. the lead.
//! * Target lead: the plan's
//!   `T_client = T_serveur + ⌈(rtt/2 + 2j)/Δt⌉ + m`, with `T_serveur` the
//!   server tick being processed now (`S - 1 + rtt/(2Δt)`), gives
//!   `a_target = ⌈(rtt/2 + 2j)/Δt⌉ + m - rtt/(2Δt)`: a command reaches the
//!   server about `2j/Δt + m` ticks early.
//!
//! Synchronization (decision): the first [`ClockConfig::sync_snapshots`]
//! snapshots (default [`DEFAULT_SYNC_SNAPSHOTS`], about a third of a second)
//! only measure their arrival offset against the local real-time clock;
//! their mean, which averages out the down-link jitter (a single sample is
//! off by up to ±jitter), estimates `T_serveur`, and the client time is then
//! set to `T_client` directly. Nothing has been simulated yet, so no tick is
//! skipped. From then on the client only changes speed:
//!
//! ```text
//! timescale = 1 + clamp(k (a_target - a_measured), -0.05, 0.05)
//! ```
//!
//! Decisions: `k` = [`DEFAULT_GAIN`] (0.02 per tick of error: 2.5 ticks of
//! error reach the 5 % limit; the loop settles in about 50 ticks and the
//! filtered snapshot jitter moves the speed by well under 1 %), `m` =
//! [`DEFAULT_MARGIN_TICKS`] (2, so the first redundant copy of a lost
//! command still arrives in time).

use std::time::Duration;

use faset_core::Tick;

/// Slowest simulation speed relative to real time.
pub const MIN_TIMESCALE: f64 = 0.95;
/// Fastest simulation speed relative to real time.
pub const MAX_TIMESCALE: f64 = 1.05;
/// Default safety margin `m` in ticks (decision, the plan allows 1 or 2).
pub const DEFAULT_MARGIN_TICKS: u32 = 2;
/// Default speed gain `k` per tick of lead error (decision).
pub const DEFAULT_GAIN: f64 = 0.02;
/// Default number of snapshots measured before the clock is set (decision).
pub const DEFAULT_SYNC_SNAPSHOTS: u32 = 10;

/// Weight of a new sample in every filter of this module (the plan's 0.1).
const FILTER: f64 = 0.1;
/// Longest frame taken into account; longer frames lose the excess time,
/// which the speed modulation then recovers (same clamp as
/// `faset_sim::Accumulator`).
const MAX_FRAME: f64 = 0.25;

fn dt() -> f64 {
    f64::from(faset_core::DT)
}

/// Tuning of a [`ClockSync`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockConfig {
    /// Safety margin `m` in ticks added to the lead.
    pub margin_ticks: u32,
    /// Gain `k`: speed change per tick of lead error.
    pub gain: f64,
    /// Snapshots whose arrival is averaged before the clock is set (at
    /// least one).
    pub sync_snapshots: u32,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            margin_ticks: DEFAULT_MARGIN_TICKS,
            gain: DEFAULT_GAIN,
            sync_snapshots: DEFAULT_SYNC_SNAPSHOTS,
        }
    }
}

/// Client clock: RTT and jitter estimates, lead over the server and the
/// speed factor that keeps the lead on target.
///
/// Drive it with [`observe_rtt`](Self::observe_rtt) and
/// [`on_snapshot`](Self::on_snapshot) for every snapshot received, and
/// [`advance`](Self::advance) once per frame with the real frame time; it
/// yields the ticks to simulate, always consecutive.
#[derive(Debug, Clone)]
pub struct ClockSync {
    config: ClockConfig,
    rtt: Option<f64>,
    jitter: f64,
    /// Filtered `τ - (S - 1)` in ticks; `None` until synchronized.
    offset: Option<f64>,
    newest_snapshot: Option<Tick>,
    /// Real seconds since creation (frames are not clamped here).
    real: f64,
    /// Before synchronization: sum and count of `real/Δt - (S - 1)`.
    arrivals: (f64, u32),
    /// Next tick to simulate.
    next: u32,
    /// Scaled seconds accumulated toward `next`.
    acc: f64,
    timescale: f64,
}

impl Default for ClockSync {
    fn default() -> Self {
        Self::new(ClockConfig::default())
    }
}

impl ClockSync {
    /// An unsynchronized clock: it simulates nothing until the first
    /// snapshot.
    #[must_use]
    pub fn new(config: ClockConfig) -> Self {
        Self {
            config,
            rtt: None,
            jitter: 0.0,
            offset: None,
            newest_snapshot: None,
            real: 0.0,
            arrivals: (0.0, 0),
            next: 0,
            acc: 0.0,
            timescale: 1.0,
        }
    }

    /// The tuning.
    #[must_use]
    pub fn config(&self) -> ClockConfig {
        self.config
    }

    /// Feeds one round-trip sample in seconds. Negative or non-finite
    /// samples are ignored.
    pub fn observe_rtt(&mut self, sample: f64) {
        if !sample.is_finite() || sample < 0.0 {
            return;
        }
        match self.rtt {
            None => {
                self.rtt = Some(sample);
                self.jitter = 0.0;
            }
            Some(rtt) => {
                self.jitter += FILTER * ((sample - rtt).abs() - self.jitter);
                self.rtt = Some(rtt + FILTER * (sample - rtt));
            }
        }
    }

    /// Smoothed round-trip time in seconds (0 before any sample).
    #[must_use]
    pub fn rtt(&self) -> f64 {
        self.rtt.unwrap_or(0.0)
    }

    /// Smoothed jitter in seconds.
    #[must_use]
    pub fn jitter(&self) -> f64 {
        self.jitter
    }

    /// Whether the clock has been set.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.offset.is_some()
    }

    /// `a_target` in ticks: `⌈(rtt/2 + 2j)/Δt⌉ + m - rtt/(2Δt)`.
    #[must_use]
    pub fn target_lead(&self) -> f64 {
        let half = self.rtt() / 2.0;
        ((half + 2.0 * self.jitter) / dt()).ceil() + f64::from(self.config.margin_ticks)
            - half / dt()
    }

    /// `a_measured` in ticks, once synced.
    #[must_use]
    pub fn measured_lead(&self) -> Option<f64> {
        Some(self.offset? - self.rtt() / dt())
    }

    /// Current speed factor, always in [[`MIN_TIMESCALE`], [`MAX_TIMESCALE`]].
    #[must_use]
    pub fn timescale(&self) -> f64 {
        self.timescale
    }

    /// The next tick [`advance`](Self::advance) will yield.
    #[must_use]
    pub fn next_tick(&self) -> Tick {
        Tick(self.next)
    }

    /// Client time `τ` in ticks: the last simulated tick plus the fraction
    /// of the next one already accumulated.
    #[must_use]
    pub fn time(&self) -> f64 {
        f64::from(self.next) - 1.0 + self.acc / dt()
    }

    /// Takes the arrival of the snapshot of server tick `tick` (call it as
    /// soon as the snapshot is decoded, after [`observe_rtt`](Self::observe_rtt)).
    /// The first [`ClockConfig::sync_snapshots`] are averaged, then the
    /// client time is set to `T_client`; later ones refine the measured
    /// lead. Duplicate and out-of-order snapshots are ignored. Returns `true`
    /// when this call synchronized the clock.
    pub fn on_snapshot(&mut self, tick: Tick) -> bool {
        if self.newest_snapshot.is_some_and(|n| tick <= n) {
            return false;
        }
        self.newest_snapshot = Some(tick);
        let sent_at = f64::from(tick.0) - 1.0;
        match self.offset {
            None => {
                let now = self.real / dt();
                self.arrivals.0 += now - sent_at;
                self.arrivals.1 += 1;
                if self.arrivals.1 < self.config.sync_snapshots.max(1) {
                    return false;
                }
                let arrival = self.arrivals.0 / f64::from(self.arrivals.1);
                // T_serveur = (S - 1) + rtt/(2Δt), with the arrival delay
                // averaged; τ = T_client = T_serveur + ⌈(rtt/2 + 2j)/Δt⌉ + m.
                let server = now - arrival + self.rtt() / (2.0 * dt());
                let target = (server + self.target_lead() + self.rtt() / (2.0 * dt())).max(0.0);
                self.next = target.floor() as u32 + 1;
                self.acc = target.fract() * dt();
                // Filtered τ - (S - 1) consistent with the averaged arrivals.
                self.offset = Some(arrival + target - now);
                tracing::debug!(tick = tick.0, client = target, "clock synchronized");
                true
            }
            Some(offset) => {
                let sample = self.time() - sent_at;
                self.offset = Some(offset + FILTER * (sample - offset));
                false
            }
        }
    }

    /// Advances by one frame of `real` time and returns the ticks to
    /// simulate now, in order. Nothing runs before synchronization. The
    /// speed factor is updated from the lead error first.
    pub fn advance(&mut self, real: Duration) -> impl Iterator<Item = Tick> {
        let first = self.next;
        let mut count = 0u32;
        let real = real.as_secs_f64();
        self.real += real;
        if let Some(offset) = self.offset {
            let error = self.target_lead() - (offset - self.rtt() / dt());
            self.timescale =
                1.0 + (self.config.gain * error).clamp(MIN_TIMESCALE - 1.0, MAX_TIMESCALE - 1.0);
            let simulated = real.min(MAX_FRAME) * self.timescale;
            // The server advanced by `real`, the client by `simulated`.
            self.offset = Some(offset + (simulated - real) / dt());
            self.acc += simulated;
            while self.acc >= dt() {
                self.acc -= dt();
                count += 1;
            }
            self.next = first.wrapping_add(count);
        }
        (0..count).map(move |i| Tick(first.wrapping_add(i)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rtt_and_jitter_follow_the_plan_filter() {
        let mut c = ClockSync::default();
        assert_eq!(c.rtt(), 0.0);
        c.observe_rtt(0.100);
        assert!((c.rtt() - 0.100).abs() < 1e-12);
        assert_eq!(c.jitter(), 0.0);
        c.observe_rtt(0.200);
        assert!((c.jitter() - 0.010).abs() < 1e-12, "{}", c.jitter());
        assert!((c.rtt() - 0.110).abs() < 1e-12, "{}", c.rtt());
        c.observe_rtt(f64::NAN);
        c.observe_rtt(-1.0);
        assert!((c.rtt() - 0.110).abs() < 1e-12);
    }

    #[test]
    fn target_lead_matches_the_formula() {
        let c = ClockSync {
            rtt: Some(0.1),
            jitter: 0.005,
            ..ClockSync::default()
        };
        // ⌈(0.05 + 0.01) * 60⌉ = 4; + 2 - 3 = 3 ticks.
        assert!((c.target_lead() - 3.0).abs() < 1e-4, "{}", c.target_lead());
    }

    /// A clock set by its first snapshot.
    fn quick() -> ClockSync {
        ClockSync::new(ClockConfig {
            sync_snapshots: 1,
            ..ClockConfig::default()
        })
    }

    #[test]
    fn first_snapshot_sets_the_client_ahead() {
        let mut c = quick();
        assert_eq!(c.advance(Duration::from_millis(100)).count(), 0);
        c.real = 0.0;
        c.observe_rtt(0.1);
        assert!(c.on_snapshot(Tick(100)));
        // 99 + 3 (rtt/2) + ⌈3⌉ + 2 = 107.
        assert!((c.time() - 107.0).abs() < 1e-3, "{}", c.time());
        assert_eq!(c.next_tick(), Tick(c.time().floor() as u32 + 1));
        let lead = c.measured_lead().unwrap();
        assert!((lead - c.target_lead()).abs() < 1e-9);
        // Stale and duplicate snapshots are ignored.
        assert!(!c.on_snapshot(Tick(100)));
        assert!(!c.on_snapshot(Tick(90)));
    }

    #[test]
    fn synchronization_averages_the_arrival_jitter() {
        let mut c = ClockSync::default();
        c.observe_rtt(0.1);
        // Snapshots arrive 3 ticks after leaving, ±1 tick of jitter.
        for i in 0..DEFAULT_SYNC_SNAPSHOTS {
            let s = 100 + 2 * i;
            let jitter = if i % 2 == 0 { 1.0 } else { -1.0 };
            c.real = (f64::from(s) - 1.0 + 3.0 + jitter) * dt();
            assert_eq!(c.on_snapshot(Tick(s)), i + 1 == DEFAULT_SYNC_SNAPSHOTS);
        }
        // Server now = real time (the mean delay is exactly rtt/2), so the
        // client starts ⌈3⌉ + 2 ticks ahead of it.
        let want = c.real / dt() + 5.0;
        assert!((c.time() - want).abs() < 1e-3, "{} vs {want}", c.time());
        assert!((c.measured_lead().unwrap() - c.target_lead()).abs() < 1e-9);
    }

    #[test]
    fn large_errors_saturate_the_speed() {
        let mut c = quick();
        c.on_snapshot(Tick(10));
        // The client believes it is 50 ticks too far ahead: slow down.
        c.offset = c.offset.map(|o| o + 50.0);
        let _ = c.advance(Duration::from_millis(16)).count();
        assert_eq!(c.timescale(), MIN_TIMESCALE);
        c.offset = c.offset.map(|o| o - 100.0);
        let _ = c.advance(Duration::from_millis(16)).count();
        assert_eq!(c.timescale(), MAX_TIMESCALE);
    }

    /// A server at real speed behind a symmetric link of one-way delay `d`;
    /// the client must converge on the target lead, measured against the
    /// true arrival times, with the speed always within 5 %.
    fn simulate(rtt_before: f64, rtt_after: f64) -> (f64, f64) {
        let dt = dt();
        let mut c = ClockSync::default();
        let frame = Duration::from_millis(1);
        let mut now = 0.0f64;
        let mut sent: Vec<(Tick, f64)> = Vec::new();
        let mut next_tick: Option<Tick> = None;
        for ms in 0..20_000u32 {
            let rtt = if ms < 5_000 { rtt_before } else { rtt_after };
            let d = rtt / 2.0;
            now += frame.as_secs_f64();
            // Snapshots of every second server tick, already arrived.
            let processing = ((now - d) / dt).floor();
            if processing >= 0.0 && (processing as u32).is_multiple_of(2) {
                c.observe_rtt(rtt);
                c.on_snapshot(Tick(processing as u32 + 1));
            }
            for t in c.advance(frame) {
                if let Some(n) = next_tick {
                    assert_eq!(t, n, "skipped or repeated a tick");
                }
                next_tick = Some(t.next());
                sent.push((t, now));
            }
            assert!((MIN_TIMESCALE..=MAX_TIMESCALE).contains(&c.timescale()));
        }
        // True lead of the last second of commands: consumption time minus
        // arrival time, averaged.
        let recent: Vec<f64> = sent
            .iter()
            .rev()
            .take(60)
            .map(|&(t, at)| (f64::from(t.0) * dt - (at + rtt_after / 2.0)) / dt)
            .collect();
        let lead = recent.iter().sum::<f64>() / recent.len() as f64;
        (lead, c.target_lead())
    }

    #[test]
    fn lead_converges_and_follows_a_latency_change() {
        let (lead, target) = simulate(0.1, 0.2);
        // Frame and snapshot granularity: within one tick of the target.
        assert!((lead - target).abs() < 1.0, "lead {lead} target {target}");
        assert!(lead >= 1.0, "commands would arrive late: {lead}");
        let (lead, target) = simulate(0.2, 0.05);
        assert!((lead - target).abs() < 1.0, "lead {lead} target {target}");
    }

    proptest! {
        #[test]
        fn speed_is_bounded_and_ticks_are_consecutive(
            events in proptest::collection::vec((0u32..40, 0.0f64..0.5, 0u64..60), 1..200),
        ) {
            let mut c = ClockSync::default();
            let mut server = 0u32;
            let mut next: Option<Tick> = None;
            for (jump, rtt, frame_ms) in events {
                server += jump;
                c.observe_rtt(rtt);
                c.on_snapshot(Tick(server));
                for t in c.advance(Duration::from_millis(frame_ms)) {
                    if let Some(n) = next {
                        prop_assert_eq!(t, n);
                    }
                    next = Some(t.next());
                }
                prop_assert!((MIN_TIMESCALE..=MAX_TIMESCALE).contains(&c.timescale()));
            }
        }
    }
}
