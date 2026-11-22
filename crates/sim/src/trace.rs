//! Input traces: record per-tick inputs, replay them in a fresh world and
//! compare final positions and state hash.
//!
//! Text format (one item per line):
//! ```text
//! seed <u64>
//! player <netid> <x> <y> <z>          (f32 bit patterns, hex)
//! frame                               (starts a tick)
//! in <netid> <tick> <buttons> <mx> <my> <yaw> <pitch> <slot>
//! hash <hex u64>                      (expected final state hash)
//! pos <netid> <x> <y> <z>             (expected final positions, f32 bits hex)
//! ```

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_protocol::InputCommand;

use crate::collision::StaticWorld;
use crate::controller::MoveParams;
use crate::rng::Pcg32;
use crate::world::World;

#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub seed: u64,
    pub players: Vec<(NetId, Vec3)>,
    pub frames: Vec<Vec<(NetId, InputCommand)>>,
}

/// Final state of a replay.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub hash: u64,
    /// Ascending `NetId`.
    pub positions: Vec<(NetId, Vec3)>,
}

#[derive(Debug, thiserror::Error)]
#[error("trace parse error in line: {0}")]
pub struct ParseError(pub String);

impl Trace {
    /// Pseudo-random trace for the test arena, deterministic from `seed`.
    #[must_use]
    pub fn generate(seed: u64, ticks: u32, players: u16) -> Self {
        let mut rng = Pcg32::new(seed, 1);
        let list: Vec<(NetId, Vec3)> = (0..players)
            .map(|i| {
                (
                    NetId(i + 1),
                    Vec3::new(-20.0 + f32::from(i) * 3.0, 0.91, 0.0),
                )
            })
            .collect();
        let mut frames = Vec::new();
        for t in 0..ticks {
            let mut f = Vec::new();
            for (id, _) in &list {
                f.push((
                    *id,
                    InputCommand {
                        tick: Tick(t),
                        buttons: (rng.next_u32() & 0xF) as u16,
                        move_x: (rng.next_u32() & 0xFF) as u8 as i8,
                        move_y: (rng.next_u32() & 0xFF) as u8 as i8,
                        yaw: (rng.next_u32() & 0xFFFF) as u16,
                        pitch: 0,
                        weapon_slot: 0,
                    },
                ));
            }
            frames.push(f);
        }
        Self {
            seed,
            players: list,
            frames,
        }
    }

    /// Replays the trace in a fresh world on the test arena.
    #[must_use]
    pub fn replay(&self) -> Outcome {
        self.run(World::with_statics(self.seed, StaticWorld::test_arena()))
    }

    /// Replays the trace in a fresh world on `statics` with the character
    /// controller (lot 4.2).
    #[must_use]
    pub fn replay_controller(&self, statics: &StaticWorld, params: MoveParams) -> Outcome {
        let w = World::with_controller(self.seed, statics.clone(), params);
        self.run(w)
    }

    fn run(&self, mut w: World) -> Outcome {
        for (id, p) in &self.players {
            w.spawn_player(*id, *p);
        }
        for f in &self.frames {
            w.step(f);
        }
        let mut ids: Vec<NetId> = self.players.iter().map(|(i, _)| *i).collect();
        ids.sort();
        Outcome {
            hash: w.state_hash(),
            positions: ids
                .into_iter()
                .filter_map(|i| w.position(i).map(|p| (i, p)))
                .collect(),
        }
    }

    /// Serialises the trace together with the expected outcome.
    #[must_use]
    pub fn to_text(&self, expected: &Outcome) -> String {
        let bits = |v: Vec3| {
            format!(
                "{:08x} {:08x} {:08x}",
                v.x.to_bits(),
                v.y.to_bits(),
                v.z.to_bits()
            )
        };
        let mut s = format!("seed {}\n", self.seed);
        for (id, p) in &self.players {
            s += &format!("player {} {}\n", id.0, bits(*p));
        }
        for f in &self.frames {
            s += "frame\n";
            for (id, c) in f {
                s += &format!(
                    "in {} {} {} {} {} {} {} {}\n",
                    id.0, c.tick.0, c.buttons, c.move_x, c.move_y, c.yaw, c.pitch, c.weapon_slot
                );
            }
        }
        s += &format!("hash {:016x}\n", expected.hash);
        for (id, p) in &expected.positions {
            s += &format!("pos {} {}\n", id.0, bits(*p));
        }
        s
    }

    /// Parses [`Trace::to_text`] output.
    ///
    /// # Errors
    /// [`ParseError`] on malformed input.
    pub fn from_text(text: &str) -> Result<(Self, Outcome), ParseError> {
        let err = |l: &str| ParseError(l.to_owned());
        let f32h = |s: &str| {
            u32::from_str_radix(s, 16)
                .map(f32::from_bits)
                .map_err(|_| err(s))
        };
        let mut trace = Self {
            seed: 0,
            players: vec![],
            frames: vec![],
        };
        let mut out = Outcome {
            hash: 0,
            positions: vec![],
        };
        for line in text.lines() {
            let p: Vec<&str> = line.split_whitespace().collect();
            match p.as_slice() {
                ["seed", s] => trace.seed = s.parse().map_err(|_| err(line))?,
                ["player", id, x, y, z] => trace.players.push((
                    NetId(id.parse().map_err(|_| err(line))?),
                    Vec3::new(f32h(x)?, f32h(y)?, f32h(z)?),
                )),
                ["frame"] => trace.frames.push(vec![]),
                ["in", id, tick, b, mx, my, yaw, pitch, slot] => {
                    let c = InputCommand {
                        tick: Tick(tick.parse().map_err(|_| err(line))?),
                        buttons: b.parse().map_err(|_| err(line))?,
                        move_x: mx.parse().map_err(|_| err(line))?,
                        move_y: my.parse().map_err(|_| err(line))?,
                        yaw: yaw.parse().map_err(|_| err(line))?,
                        pitch: pitch.parse().map_err(|_| err(line))?,
                        weapon_slot: slot.parse().map_err(|_| err(line))?,
                    };
                    let id = NetId(id.parse().map_err(|_| err(line))?);
                    trace
                        .frames
                        .last_mut()
                        .ok_or_else(|| err(line))?
                        .push((id, c));
                }
                ["hash", h] => out.hash = u64::from_str_radix(h, 16).map_err(|_| err(line))?,
                ["pos", id, x, y, z] => out.positions.push((
                    NetId(id.parse().map_err(|_| err(line))?),
                    Vec3::new(f32h(x)?, f32h(y)?, f32h(z)?),
                )),
                [] => {}
                _ => return Err(err(line)),
            }
        }
        Ok((trace, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = include_str!("../tests/data/reference.trace");

    #[test]
    fn replay_is_repeatable() {
        let t = Trace::generate(7, 300, 3);
        assert_eq!(t.replay(), t.replay());
    }

    #[test]
    fn text_round_trips() {
        let t = Trace::generate(3, 20, 2);
        let o = t.replay();
        let (t2, o2) = Trace::from_text(&t.to_text(&o)).unwrap();
        assert_eq!((t2, o2), (t, o));
    }

    /// The committed reference trace replays to identical positions and hash.
    #[test]
    fn reference_trace_replays_identically() {
        let (trace, expected) = Trace::from_text(REFERENCE).unwrap();
        assert!(!trace.frames.is_empty());
        assert_eq!(trace.replay(), expected);
    }

    /// Regenerates the reference file: `cargo test -- --ignored regenerate`.
    #[test]
    #[ignore]
    fn regenerate_reference() {
        let t = Trace::generate(2024, 1200, 4);
        let o = t.replay();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/reference.trace");
        std::fs::write(path, t.to_text(&o)).unwrap();
    }
}
