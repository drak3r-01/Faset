//! Real-time pacing of the tick loop.

use std::time::{Duration, Instant};

/// Most ticks run in one loop iteration when the server falls behind; the
/// rest of the backlog is dropped.
pub const MAX_CATCH_UP: u32 = 5;

/// The last part of each wait is a busy loop of this length, because
/// `sleep` alone overshoots by up to a scheduler quantum.
const SPIN: Duration = Duration::from_millis(1);

/// Paces ticks at a fixed rate: sleep until about 1 ms before the next tick,
/// then wait actively.
#[derive(Debug, Clone)]
pub struct TickClock {
    period: Duration,
    next: Instant,
}

impl TickClock {
    /// A clock at `rate` Hz whose first tick is due now.
    ///
    /// # Panics
    /// If `rate` is zero.
    #[must_use]
    pub fn new(rate: u8) -> Self {
        Self::starting_at(rate, Instant::now())
    }

    /// A clock whose first tick is due at `start`.
    ///
    /// # Panics
    /// If `rate` is zero.
    #[must_use]
    pub fn starting_at(rate: u8, start: Instant) -> Self {
        assert!(rate > 0, "tick rate must be positive");
        Self {
            period: Duration::from_secs(1) / u32::from(rate),
            next: start,
        }
    }

    /// Time between ticks.
    #[must_use]
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Blocks until the next tick is due and returns how many ticks to run
    /// now (1 normally, up to [`MAX_CATCH_UP`] when late).
    pub fn wait(&mut self) -> u32 {
        loop {
            let now = Instant::now();
            if now >= self.next {
                return self.due(now);
            }
            let left = self.next - now;
            if left > SPIN {
                std::thread::sleep(left - SPIN);
            } else {
                std::hint::spin_loop();
            }
        }
    }

    /// Ticks due at `now` (>= the next deadline), capped at
    /// [`MAX_CATCH_UP`]; advances the deadline accordingly. When the backlog
    /// exceeds the cap the extra ticks are dropped and the next deadline is
    /// one period from `now`.
    fn due(&mut self, now: Instant) -> u32 {
        let late = now.saturating_duration_since(self.next);
        let behind = late.as_nanos() / self.period.as_nanos() + 1;
        let max = u128::from(MAX_CATCH_UP);
        if behind > max {
            self.next = now + self.period;
            MAX_CATCH_UP
        } else {
            let n = behind as u32;
            self.next += self.period * n;
            n
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_time_runs_one_tick() {
        let t0 = Instant::now();
        let mut c = TickClock::starting_at(60, t0);
        assert_eq!(c.due(t0), 1);
        assert_eq!(c.next, t0 + c.period());
    }

    #[test]
    fn catch_up_is_capped_at_five() {
        let t0 = Instant::now();
        let mut c = TickClock::starting_at(60, t0);
        let p = c.period();
        // Three periods late: four ticks due.
        assert_eq!(c.due(t0 + p * 3), 4);
        assert_eq!(c.next, t0 + p * 4);
        // A second behind: capped, backlog dropped.
        let late = t0 + Duration::from_secs(1);
        assert_eq!(c.due(late), MAX_CATCH_UP);
        assert_eq!(c.next, late + p);
    }

    #[test]
    fn wait_paces_at_rate() {
        let start = Instant::now();
        let mut c = TickClock::starting_at(60, start);
        let mut ticks = 0;
        while ticks < 12 {
            ticks += c.wait();
        }
        let elapsed = start.elapsed();
        // 12 ticks: the first is immediate, 11 periods follow (183 ms).
        assert!(elapsed >= c.period() * 11, "{elapsed:?}");
    }
}
