//! Network condition simulator: latency, jitter, loss, duplication, reordering.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::time::Duration;

use crate::transport::{check_size, Datagram, Transport, TransportError};

/// Parameters of the simulated link. Applied to every packet *sent* through
/// the wrapper; wrap both ends to degrade both directions.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkConditions {
    /// Fixed one-way delay.
    pub latency: Duration,
    /// Each packet's delay varies uniformly in `latency ± jitter`.
    pub jitter: Duration,
    /// Probability in percent (0 to 100) that a packet is dropped.
    pub loss_percent: f32,
    /// Probability in percent that a packet is delivered twice.
    pub duplicate_percent: f32,
    /// Probability in percent that a packet is held back by
    /// [`reorder_delay`](Self::reorder_delay), letting later packets overtake it.
    pub reorder_percent: f32,
    /// Extra delay applied to a reordered packet.
    pub reorder_delay: Duration,
    /// Seed of the random generator: equal seeds give equal behaviour.
    pub seed: u64,
}

impl Default for NetworkConditions {
    /// A perfect link: no delay, no loss.
    fn default() -> Self {
        Self {
            latency: Duration::ZERO,
            jitter: Duration::ZERO,
            loss_percent: 0.0,
            duplicate_percent: 0.0,
            reorder_percent: 0.0,
            reorder_delay: Duration::from_millis(50),
            seed: 0x5EED,
        }
    }
}

impl NetworkConditions {
    /// The profile used by integration tests elsewhere: 100 ms latency, 20 ms
    /// jitter, 5 % loss.
    #[must_use]
    pub fn typical() -> Self {
        Self {
            latency: Duration::from_millis(100),
            jitter: Duration::from_millis(20),
            loss_percent: 5.0,
            ..Self::default()
        }
    }
}

/// SplitMix64: tiny, deterministic, good enough for packet chaos.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` with 53 bits of precision.
    fn unit(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let bits = (self.next_u64() >> 11) as f64;
        bits / 9_007_199_254_740_992.0
    }

    fn chance_percent(&mut self, percent: f32) -> bool {
        percent > 0.0 && self.unit() * 100.0 < f64::from(percent)
    }
}

#[derive(Debug)]
struct Pending<A> {
    due: Duration,
    seq: u64,
    to: A,
    data: Vec<u8>,
}

impl<A> PartialEq for Pending<A> {
    fn eq(&self, other: &Self) -> bool {
        (self.due, self.seq) == (other.due, other.seq)
    }
}
impl<A> Eq for Pending<A> {}
impl<A> PartialOrd for Pending<A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<A> Ord for Pending<A> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.due, self.seq).cmp(&(other.due, other.seq))
    }
}

/// Wraps any [`Transport`] and degrades the packets sent through it.
///
/// Delayed packets are released by [`Transport::update`], which advances a
/// virtual clock; the wrapper never reads the wall clock, so simulations are
/// reproducible and run faster than real time.
#[derive(Debug)]
pub struct ConditionedTransport<T: Transport> {
    inner: T,
    conditions: NetworkConditions,
    rng: Rng,
    now: Duration,
    seq: u64,
    queue: BinaryHeap<Reverse<Pending<T::Addr>>>,
}

impl<T: Transport> ConditionedTransport<T> {
    /// Wraps `inner` with the given conditions.
    #[must_use]
    pub fn new(inner: T, conditions: NetworkConditions) -> Self {
        Self {
            rng: Rng(conditions.seed),
            inner,
            conditions,
            now: Duration::ZERO,
            seq: 0,
            queue: BinaryHeap::new(),
        }
    }

    /// The wrapped transport.
    #[must_use]
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Mutable access to the wrapped transport.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Current conditions.
    #[must_use]
    pub fn conditions(&self) -> &NetworkConditions {
        &self.conditions
    }

    /// Replaces the conditions; packets already in flight keep their delay.
    pub fn set_conditions(&mut self, conditions: NetworkConditions) {
        self.conditions = conditions;
    }

    /// Number of packets currently delayed inside the simulator.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.queue.len()
    }

    fn delay(&mut self) -> Duration {
        let c = &self.conditions;
        let spread = c.jitter.as_secs_f64() * (self.rng.unit() * 2.0 - 1.0);
        let mut secs = c.latency.as_secs_f64() + spread;
        if self.rng.chance_percent(self.conditions.reorder_percent) {
            secs += self.conditions.reorder_delay.as_secs_f64();
        }
        Duration::from_secs_f64(secs.max(0.0))
    }

    fn enqueue(&mut self, to: T::Addr, packet: &[u8]) -> Result<(), TransportError> {
        let delay = self.delay();
        if delay.is_zero() {
            return self.inner.send_to(to, packet);
        }
        self.seq += 1;
        self.queue.push(Reverse(Pending {
            due: self.now + delay,
            seq: self.seq,
            to,
            data: packet.to_vec(),
        }));
        Ok(())
    }
}

impl<T: Transport> Transport for ConditionedTransport<T> {
    type Addr = T::Addr;

    fn send_to(&mut self, to: T::Addr, packet: &[u8]) -> Result<(), TransportError> {
        check_size(packet)?;
        if self.rng.chance_percent(self.conditions.loss_percent) {
            return Ok(());
        }
        self.enqueue(to, packet)?;
        if self.rng.chance_percent(self.conditions.duplicate_percent) {
            self.enqueue(to, packet)?;
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Datagram<T::Addr>>, TransportError> {
        self.inner.poll()
    }

    fn update(&mut self, dt: Duration) {
        self.inner.update(dt);
        self.now += dt;
        while self
            .queue
            .peek()
            .is_some_and(|Reverse(p)| p.due <= self.now)
        {
            let Some(Reverse(p)) = self.queue.pop() else {
                break;
            };
            if let Err(e) = self.inner.send_to(p.to, &p.data) {
                tracing::warn!(error = %e, "conditioned transport failed to release a packet");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryTransport, MAX_PACKET_SIZE};

    fn run(conditions: NetworkConditions, packets: usize) -> (Vec<(Duration, u32)>, Duration) {
        let (a, mut b) = MemoryTransport::pair();
        let to = b.addr();
        let mut a = ConditionedTransport::new(a, conditions);
        let step = Duration::from_millis(1);
        let mut now = Duration::ZERO;
        let mut got = Vec::new();
        let mut sent = 0u32;
        for _ in 0..(packets * 20 + 2000) {
            if (sent as usize) < packets {
                a.send_to(to, &sent.to_le_bytes()).unwrap();
                sent += 1;
            }
            a.update(step);
            now += step;
            while let Some(d) = b.poll().unwrap() {
                got.push((now, u32::from_le_bytes(d.data.try_into().unwrap())));
            }
        }
        (got, now)
    }

    #[test]
    fn perfect_link_is_a_passthrough() {
        let (got, _) = run(NetworkConditions::default(), 50);
        let ids: Vec<u32> = got.iter().map(|g| g.1).collect();
        assert_eq!(ids, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn latency_delays_delivery() {
        let (got, _) = run(
            NetworkConditions {
                latency: Duration::from_millis(100),
                ..NetworkConditions::default()
            },
            5,
        );
        assert_eq!(got.len(), 5);
        // Packet i is sent at step i and arrives 100 ms later, not before.
        for (now, id) in got {
            assert!(now >= Duration::from_millis(100 + u64::from(id)));
        }
    }

    #[test]
    fn loss_rate_matches_configuration() {
        let (got, _) = run(
            NetworkConditions {
                loss_percent: 10.0,
                ..NetworkConditions::default()
            },
            4000,
        );
        let lost = 4000 - got.len();
        // Mean 400, sd ~19.
        assert!((300..500).contains(&lost), "lost {lost}");
    }

    #[test]
    fn duplication_delivers_extra_copies() {
        let (got, _) = run(
            NetworkConditions {
                duplicate_percent: 25.0,
                ..NetworkConditions::default()
            },
            2000,
        );
        let extra = got.len() - 2000;
        assert!((400..600).contains(&extra), "extra {extra}");
    }

    #[test]
    fn jitter_and_reordering_change_arrival_order() {
        let (got, _) = run(
            NetworkConditions {
                latency: Duration::from_millis(50),
                reorder_percent: 20.0,
                ..NetworkConditions::default()
            },
            500,
        );
        assert_eq!(got.len(), 500);
        let ids: Vec<u32> = got.iter().map(|g| g.1).collect();
        assert!(ids.windows(2).any(|w| w[1] < w[0]), "no reordering seen");
        let mut sorted = ids;
        sorted.sort_unstable();
        assert_eq!(sorted, (0..500).collect::<Vec<_>>());
    }

    #[test]
    fn same_seed_same_behaviour() {
        let c = NetworkConditions {
            latency: Duration::from_millis(20),
            jitter: Duration::from_millis(10),
            loss_percent: 15.0,
            duplicate_percent: 5.0,
            reorder_percent: 10.0,
            ..NetworkConditions::default()
        };
        assert_eq!(run(c.clone(), 300).0, run(c, 300).0);
    }

    #[test]
    fn rejects_oversized_packets() {
        let (a, b) = MemoryTransport::pair();
        let mut a = ConditionedTransport::new(a, NetworkConditions::default());
        let big = vec![0; MAX_PACKET_SIZE + 1];
        assert!(a.send_to(b.addr(), &big).is_err());
    }
}
