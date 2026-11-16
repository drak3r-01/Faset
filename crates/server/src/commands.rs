//! Per-client input command buffer.

use std::collections::BTreeMap;

use faset_core::Tick;
use faset_protocol::InputCommand;

/// Commands of one client, ordered by tick.
///
/// Commands for past ticks or too far in the future are refused. At the
/// start of each tick [`take`](Self::take) returns the command stamped for
/// that tick, or repeats the last one when it is missing.
///
/// Ticks are compared as plain `u32` (wrap-around takes 2.2 years at 60 Hz).
#[derive(Debug, Clone)]
pub struct CommandBuffer {
    pending: BTreeMap<u32, InputCommand>,
    last: Option<InputCommand>,
    last_processed: Option<Tick>,
    horizon: u32,
}

impl CommandBuffer {
    /// A buffer accepting commands up to `horizon` ticks ahead.
    #[must_use]
    pub fn new(horizon: u32) -> Self {
        Self {
            pending: BTreeMap::new(),
            last: None,
            last_processed: None,
            horizon,
        }
    }

    /// Buffers `cmd` if its tick lies in `[current, current + horizon]`.
    /// A duplicate for an already buffered tick is ignored. Returns whether
    /// the command was kept.
    pub fn push(&mut self, cmd: InputCommand, current: Tick) -> bool {
        let t = cmd.tick.0;
        if t < current.0 || t - current.0 > self.horizon {
            return false;
        }
        if self.pending.contains_key(&t) {
            return false;
        }
        self.pending.insert(t, cmd);
        true
    }

    /// The command to apply at tick `current`: the one stamped for it, else
    /// a repeat of the last one applied, else `None` (no command yet).
    /// Commands for earlier ticks are discarded.
    pub fn take(&mut self, current: Tick) -> Option<InputCommand> {
        self.pending = self.pending.split_off(&current.0);
        if let Some(cmd) = self.pending.remove(&current.0) {
            self.last = Some(cmd);
            self.last_processed = Some(current);
        }
        self.last
    }

    /// Tick of the last command actually received and applied (repeats do
    /// not count). Carried in every snapshot for that client.
    #[must_use]
    pub fn last_processed(&self) -> Option<Tick> {
        self.last_processed
    }

    /// Number of commands waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether no command is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(tick: u32, move_y: i8) -> InputCommand {
        InputCommand {
            tick: Tick(tick),
            buttons: 0,
            move_x: 0,
            move_y,
            yaw: 0,
            pitch: 0,
            weapon_slot: 0,
        }
    }

    #[test]
    fn orders_by_tick_and_repeats_missing() {
        let mut b = CommandBuffer::new(8);
        assert!(b.push(cmd(12, 3), Tick(10)));
        assert!(b.push(cmd(10, 1), Tick(10)));
        assert_eq!(b.take(Tick(10)), Some(cmd(10, 1)));
        assert_eq!(b.last_processed(), Some(Tick(10)));
        // Tick 11 missing: repeat the last command.
        assert_eq!(b.take(Tick(11)), Some(cmd(10, 1)));
        assert_eq!(b.last_processed(), Some(Tick(10)));
        assert_eq!(b.take(Tick(12)), Some(cmd(12, 3)));
        assert_eq!(b.last_processed(), Some(Tick(12)));
        assert!(b.is_empty());
    }

    #[test]
    fn refuses_late_far_and_duplicate() {
        let mut b = CommandBuffer::new(4);
        assert_eq!(b.take(Tick(5)), None);
        assert!(!b.push(cmd(4, 0), Tick(5)));
        assert!(!b.push(cmd(10, 0), Tick(5)));
        assert!(b.push(cmd(9, 0), Tick(5)));
        assert!(!b.push(cmd(9, 7), Tick(5)));
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn skipped_ticks_are_dropped() {
        let mut b = CommandBuffer::new(8);
        b.push(cmd(3, 1), Tick(2));
        b.push(cmd(4, 2), Tick(2));
        assert_eq!(b.take(Tick(4)), Some(cmd(4, 2)));
        assert!(b.is_empty());
    }
}
