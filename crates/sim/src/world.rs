//! The simulation world and its ordered per-tick systems.

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_protocol::InputCommand;
use hecs::Entity;

use crate::collision::StaticWorld;
use crate::components::{
    Health, Inventory, PlayerState, Team, Transform, Velocity, WeaponState, STANDING_HEIGHT,
};

/// Stub walk speed in m/s (real movement equations arrive with lots 4.x).
const STUB_SPEED: f32 = 5.0;

/// Inputs of one tick, one command per player.
pub type TickInputs = [(NetId, InputCommand)];

/// Hooks called by [`World::step`] for systems implemented in later lots.
/// Every method has an empty default, so a plain `World` works without any.
pub trait Hooks {
    /// System 7: game mode rules.
    fn game_mode(&mut self, _world: &mut World) {}
    /// System 9: record collision history (server only, lot 1.7).
    fn record_collision_history(&mut self, _world: &World) {}
    /// System 10: emit a snapshot (server, every other tick).
    fn emit_snapshot(&mut self, _world: &World) {}
}

/// No-op hooks.
pub struct NoHooks;
impl Hooks for NoHooks {}

pub struct World {
    ecs: hecs::World,
    statics: StaticWorld,
    tick: Tick,
    seed: u64,
    /// Ascending `NetId` order; never a hash table.
    players: BTreeMap<NetId, Entity>,
}

impl World {
    /// Empty world with an empty static map.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_statics(seed, StaticWorld::default())
    }

    #[must_use]
    pub fn with_statics(seed: u64, statics: StaticWorld) -> Self {
        Self {
            ecs: hecs::World::new(),
            statics,
            tick: Tick(0),
            seed,
            players: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn tick(&self) -> Tick {
        self.tick
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    #[must_use]
    pub fn statics(&self) -> &StaticWorld {
        &self.statics
    }

    /// Adds a player at `position` (capsule centre).
    pub fn spawn_player(&mut self, id: NetId, position: Vec3) {
        let e = self.ecs.spawn((
            id,
            Transform {
                position,
                ..Transform::default()
            },
            Velocity::default(),
            PlayerState::default(),
            Health(100),
            Inventory::default(),
            WeaponState::default(),
            Team::default(),
        ));
        self.players.insert(id, e);
    }

    /// Position of a player, if present.
    #[must_use]
    pub fn position(&self, id: NetId) -> Option<Vec3> {
        let e = *self.players.get(&id)?;
        self.ecs.get::<&Transform>(e).ok().map(|t| t.position)
    }

    /// Advances one tick with no hooks.
    pub fn step(&mut self, inputs: &TickInputs) {
        self.step_with(inputs, &mut NoHooks);
    }

    /// Advances one tick, running systems in the fixed order of section 5.
    pub fn step_with(&mut self, inputs: &TickInputs, hooks: &mut dyn Hooks) {
        // 1. Read the input commands of the tick (ascending NetId, stable).
        let mut cmds: Vec<(NetId, InputCommand)> = inputs.to_vec();
        cmds.sort_by_key(|(id, _)| *id);
        // 2. Player movement (stub).
        self.system_movement(&cmds);
        // 3. Weapons and firing (stub).
        // 4. Projectiles (stub).
        // 5. Damage and deaths (stub).
        // 6. Pickups and zones (stub).
        // 7. Game mode rules (hook).
        hooks.game_mode(self);
        // 8. Respawns (stub).
        // 9. Collision history (hook, server only).
        hooks.record_collision_history(self);
        self.tick = self.tick.next();
        // 10. Snapshot every other tick (hook, server only).
        if self.tick.0.is_multiple_of(2) {
            hooks.emit_snapshot(self);
        }
    }

    fn system_movement(&mut self, cmds: &[(NetId, InputCommand)]) {
        for (id, cmd) in cmds {
            let Some(&e) = self.players.get(id) else {
                continue;
            };
            // Quantized values are what both sides simulate with.
            let yaw = dequantize_yaw(cmd.yaw);
            let pitch = dequantize_pitch_command(cmd.pitch);
            let mx = f32::from(cmd.move_x) / 127.0;
            let my = f32::from(cmd.move_y) / 127.0;
            let fwd = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
            let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
            let wish = (fwd * my + right * mx).clamp_length_max(1.0) * STUB_SPEED;
            if let Ok((t, v)) = self.ecs.query_one_mut::<(&mut Transform, &mut Velocity)>(e) {
                v.0 = wish;
                t.yaw = yaw;
                t.pitch = pitch;
                t.position = self
                    .statics
                    .move_capsule(t.position, wish * DT, STANDING_HEIGHT);
            }
        }
    }

    /// FNV-1a hash of the full simulation state, iterating by ascending `NetId`.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.write(&self.tick.0.to_le_bytes());
        for (id, &e) in &self.players {
            h.write(&id.0.to_le_bytes());
            if let Ok(t) = self.ecs.get::<&Transform>(e) {
                for f in [t.position.x, t.position.y, t.position.z, t.yaw, t.pitch] {
                    h.write(&f.to_bits().to_le_bytes());
                }
            }
            if let Ok(v) = self.ecs.get::<&Velocity>(e) {
                for f in [v.0.x, v.0.y, v.0.z] {
                    h.write(&f.to_bits().to_le_bytes());
                }
            }
            if let Ok(hp) = self.ecs.get::<&Health>(e) {
                h.write(&hp.0.to_le_bytes());
            }
        }
        h.0
    }
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Count {
        modes: u32,
        history: u32,
        snaps: u32,
    }
    impl Hooks for Count {
        fn game_mode(&mut self, _: &mut World) {
            self.modes += 1;
        }
        fn record_collision_history(&mut self, _: &World) {
            self.history += 1;
        }
        fn emit_snapshot(&mut self, _: &World) {
            self.snaps += 1;
        }
    }

    #[test]
    fn hooks_run_in_order_and_snapshot_every_other_tick() {
        let mut w = World::new(1);
        let mut c = Count::default();
        for _ in 0..10 {
            w.step_with(&[], &mut c);
        }
        assert_eq!((c.modes, c.history, c.snaps), (10, 10, 5));
        assert_eq!(w.tick(), Tick(10));
    }

    #[test]
    fn movement_stops_at_wall() {
        let mut w = World::with_statics(1, StaticWorld::test_arena());
        w.spawn_player(NetId(1), Vec3::new(0.0, 0.91, 0.0));
        // Yaw of 3pi/2 faces +X.
        let yaw = faset_protocol::quant::quantize_yaw(3.0 * std::f32::consts::FRAC_PI_2);
        let cmd = InputCommand {
            tick: Tick(0),
            buttons: 0,
            move_x: 0,
            move_y: 127,
            yaw,
            pitch: 0,
            weapon_slot: 0,
        };
        for _ in 0..600 {
            w.step(&[(NetId(1), cmd)]);
        }
        let p = w.position(NetId(1)).unwrap();
        assert!(p.x > 9.0 && p.x <= 9.66, "{p}");
    }
}
