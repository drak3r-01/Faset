//! Byte layouts of the mode's messages. They travel inside
//! `ModeMessage(mode_id, bytes)`; the protocol knows none of this.
//!
//! All integers and floats are little endian.

use faset_core::glam::Vec3;
use faset_core::NetId;

use crate::damage::HitZone;

/// Flag of [`FireMsg`]: the trigger went down since the last message.
pub const FLAG_FRESH_PRESS: u8 = 1;
/// Flag of [`FireMsg`]: the shooter looks down the sights.
pub const FLAG_SIGHTED: u8 = 2;

/// Client to server: a shot.
///
/// Layout: tag 1 (1), view tick (4), view fraction (4), yaw (2), pitch (2),
/// flags (1). The shot counter is not sent: the server's own counter seeds
/// the spread, otherwise a client could pick a favourable seed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireMsg {
    /// Interpolation tick the shooter saw.
    pub view_tick: u32,
    /// Fraction from `view_tick` toward the next tick.
    pub view_fraction: f32,
    /// Camera yaw, recoil included, step 2π/65536.
    pub yaw: u16,
    /// Camera pitch, recoil included, step 2π/65536.
    pub pitch: i16,
    pub flags: u8,
}

impl FireMsg {
    const TAG: u8 = 1;
    const LEN: usize = 14;

    #[must_use]
    pub fn fresh_press(&self) -> bool {
        self.flags & FLAG_FRESH_PRESS != 0
    }

    #[must_use]
    pub fn sighted(&self) -> bool {
        self.flags & FLAG_SIGHTED != 0
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::LEN);
        out.push(Self::TAG);
        out.extend_from_slice(&self.view_tick.to_le_bytes());
        out.extend_from_slice(&self.view_fraction.to_le_bytes());
        out.extend_from_slice(&self.yaw.to_le_bytes());
        out.extend_from_slice(&self.pitch.to_le_bytes());
        out.push(self.flags);
        out
    }

    /// Inverse of [`encode`](Self::encode); `None` for any other length, tag
    /// or a non-finite fraction.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::LEN || bytes[0] != Self::TAG {
            return None;
        }
        let view_fraction = f32::from_le_bytes(bytes[5..9].try_into().ok()?);
        if !view_fraction.is_finite() {
            return None;
        }
        Some(Self {
            view_tick: u32::from_le_bytes(bytes[1..5].try_into().ok()?),
            view_fraction,
            yaw: u16::from_le_bytes(bytes[9..11].try_into().ok()?),
            pitch: i16::from_le_bytes(bytes[11..13].try_into().ok()?),
            flags: bytes[13],
        })
    }
}

/// Creation event of a projectile: the only thing replicated about it
/// (no per-frame state); every client simulates it locally from here.
///
/// The plan puts this on the protocol's event channel, but `protocol` has no
/// such variant and lot 4.5 may not add one: it travels as mode bytes
/// ([`ServerMsg::Projectile`]) inside `ModeMessage` until a lot owning
/// `protocol` moves it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileSpawn {
    pub shooter: NetId,
    /// Index of the weapon in the match's weapon list.
    pub weapon: u8,
    /// Tick of the shot command.
    pub tick: u32,
    /// Per-projectile seed (derived from the match seed and the shot).
    pub seed: u32,
    pub origin: Vec3,
    pub velocity: Vec3,
}

/// Server to clients: what a shot did, sent as mode events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServerMsg {
    /// A projectile was created (see [`ProjectileSpawn`]).
    Projectile(ProjectileSpawn),
    /// A blast went off at `centre` (visual and audio cue).
    Explosion { centre: Vec3 },
    /// `attacker` hurt `victim`.
    Hit {
        attacker: NetId,
        victim: NetId,
        /// Health actually removed.
        amount: f32,
        zone: HitZone,
    },
    /// `attacker` killed `victim` (kill feed).
    Kill { attacker: NetId, victim: NetId },
    /// `player` respawned at `position` (capsule centre).
    Respawn { player: NetId, position: Vec3 },
}

impl ServerMsg {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let push_vec = |out: &mut Vec<u8>, v: Vec3| {
            for c in v.to_array() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        };
        match *self {
            Self::Projectile(p) => {
                out.push(4);
                out.extend_from_slice(&p.shooter.0.to_le_bytes());
                out.push(p.weapon);
                out.extend_from_slice(&p.tick.to_le_bytes());
                out.extend_from_slice(&p.seed.to_le_bytes());
                push_vec(&mut out, p.origin);
                push_vec(&mut out, p.velocity);
            }
            Self::Explosion { centre } => {
                out.push(5);
                push_vec(&mut out, centre);
            }
            Self::Hit {
                attacker,
                victim,
                amount,
                zone,
            } => {
                out.push(1);
                out.extend_from_slice(&attacker.0.to_le_bytes());
                out.extend_from_slice(&victim.0.to_le_bytes());
                out.extend_from_slice(&amount.to_le_bytes());
                out.push(u8::from(zone == HitZone::Head));
            }
            Self::Kill { attacker, victim } => {
                out.push(2);
                out.extend_from_slice(&attacker.0.to_le_bytes());
                out.extend_from_slice(&victim.0.to_le_bytes());
            }
            Self::Respawn { player, position } => {
                out.push(3);
                out.extend_from_slice(&player.0.to_le_bytes());
                for c in position.to_array() {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        out
    }

    /// Inverse of [`encode`](Self::encode); `None` for malformed bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&tag, b) = bytes.split_first()?;
        let id = |i: usize| Some(NetId(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?)));
        let float = |i: usize| Some(f32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?));
        match (tag, b.len()) {
            (1, 9) => Some(Self::Hit {
                attacker: id(0)?,
                victim: id(2)?,
                amount: float(4)?,
                zone: if b[8] == 0 {
                    HitZone::Body
                } else {
                    HitZone::Head
                },
            }),
            (2, 4) => Some(Self::Kill {
                attacker: id(0)?,
                victim: id(2)?,
            }),
            (3, 14) => Some(Self::Respawn {
                player: id(0)?,
                position: Vec3::new(float(2)?, float(6)?, float(10)?),
            }),
            (4, 35) => {
                let origin = Vec3::new(float(11)?, float(15)?, float(19)?);
                let velocity = Vec3::new(float(23)?, float(27)?, float(31)?);
                if !(origin.is_finite() && velocity.is_finite()) {
                    return None;
                }
                Some(Self::Projectile(ProjectileSpawn {
                    shooter: id(0)?,
                    weapon: b[2],
                    tick: u32::from_le_bytes(b.get(3..7)?.try_into().ok()?),
                    seed: u32::from_le_bytes(b.get(7..11)?.try_into().ok()?),
                    origin,
                    velocity,
                }))
            }
            (5, 12) => {
                let centre = Vec3::new(float(0)?, float(4)?, float(8)?);
                centre.is_finite().then_some(Self::Explosion { centre })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_message_round_trips_and_rejects_garbage() {
        let m = FireMsg {
            view_tick: 123_456,
            view_fraction: 0.25,
            yaw: 40_000,
            pitch: -1_234,
            flags: FLAG_FRESH_PRESS | FLAG_SIGHTED,
        };
        assert_eq!(FireMsg::decode(&m.encode()), Some(m));
        assert!(m.fresh_press() && m.sighted());
        assert_eq!(FireMsg::decode(&[]), None);
        assert_eq!(FireMsg::decode(&m.encode()[..13]), None);
        let mut bad = m.encode();
        bad[5..9].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(FireMsg::decode(&bad), None);
        bad = m.encode();
        bad[0] = 9;
        assert_eq!(FireMsg::decode(&bad), None);
    }

    #[test]
    fn server_messages_round_trip() {
        for m in [
            ServerMsg::Hit {
                attacker: NetId(1),
                victim: NetId(2),
                amount: 33.5,
                zone: HitZone::Head,
            },
            ServerMsg::Kill {
                attacker: NetId(3),
                victim: NetId(4),
            },
            ServerMsg::Respawn {
                player: NetId(5),
                position: Vec3::new(1.0, 2.0, -3.0),
            },
            ServerMsg::Projectile(ProjectileSpawn {
                shooter: NetId(6),
                weapon: 3,
                tick: 99_999,
                seed: 0xdead_beef,
                origin: Vec3::new(1.0, 1.6, -2.0),
                velocity: Vec3::new(0.0, 5.0, -35.0),
            }),
            ServerMsg::Explosion {
                centre: Vec3::new(4.0, 0.5, 9.0),
            },
        ] {
            assert_eq!(ServerMsg::decode(&m.encode()), Some(m));
        }
        assert_eq!(ServerMsg::decode(&[2, 1]), None);
        assert_eq!(ServerMsg::decode(&[]), None);
    }
}
