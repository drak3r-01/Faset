//! Game events and match results, and their wire layout.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_protocol::ProjectileSpawn;

use crate::message::DecodeError;
use crate::phase::Phase;
use crate::services::TeamId;

/// Something that happened in a match. Replicated to clients and given to
/// [`GameMode::on_event`](crate::GameMode::on_event). New variants arrive
/// with later lots (damage, pickups), hence `non_exhaustive`.
///
/// Not `Eq` since lot 4.11: projectile and explosion events carry floats.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GameEvent {
    /// A player entered the match.
    PlayerJoined(NetId),
    /// A player left the match.
    PlayerLeft(NetId),
    /// The match entered `phase`; `end` is the tick it ends at, if timed.
    PhaseChanged { phase: Phase, end: Option<Tick> },
    /// A player's score changed to `score`.
    ScoreChanged { player: NetId, score: i32 },
    /// The `Playing` phase ended with this result.
    MatchEnded(MatchResult),
    /// The rotation moved to map `map`.
    MapChanged { map: String },
    /// A line of the event feed.
    Feed(String),
    /// A mode-defined event, opaque to the framework.
    Mode(Vec<u8>),
    /// A projectile was created (lot 4.11). The host sends it as the
    /// protocol's projectile event, not as opaque bytes.
    Projectile(ProjectileSpawn),
    /// A blast went off (lot 4.11), sent as the protocol's explosion event.
    Explosion {
        owner: NetId,
        /// Index of the weapon in the mode's weapon list.
        weapon: u8,
        centre: Vec3,
    },
}

/// Why the `Playing` phase ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The mode's win condition was met.
    WinCondition,
    /// The time limit ran out (winners from the score table).
    TimeLimit,
}

/// The outcome of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    /// Winning players, several on a tie; empty for a draw with no score.
    pub winners: Vec<NetId>,
    /// Winning team, for team modes.
    pub winning_team: Option<TeamId>,
    /// Why the match ended.
    pub reason: EndReason,
}

impl MatchResult {
    /// One player won by the win condition.
    #[must_use]
    pub fn winner(player: NetId) -> Self {
        Self {
            winners: vec![player],
            winning_team: None,
            reason: EndReason::WinCondition,
        }
    }
}

const G_JOINED: u8 = 0;
const G_LEFT: u8 = 1;
const G_PHASE: u8 = 2;
const G_SCORE: u8 = 3;
const G_ENDED: u8 = 4;
const G_MAP: u8 = 5;
const G_FEED: u8 = 6;
const G_MODE: u8 = 7;
const G_PROJECTILE: u8 = 8;
const G_EXPLOSION: u8 = 9;

fn push_vec3(out: &mut Vec<u8>, v: Vec3) {
    for c in v.to_array() {
        out.extend_from_slice(&c.to_le_bytes());
    }
}

fn push_text(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let len = bytes.len().min(usize::from(u16::MAX));
    out.extend_from_slice(&(len as u16).to_le_bytes());
    out.extend_from_slice(&bytes[..len]);
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        let f = f32::from_bits(self.u32()?);
        if f.is_finite() {
            Ok(f)
        } else {
            Err(DecodeError::Invalid)
        }
    }
    fn vec3(&mut self) -> Result<Vec3, DecodeError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn text(&mut self) -> Result<String, DecodeError> {
        let len = usize::from(self.u16()?);
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| DecodeError::Invalid)
    }
}

impl GameEvent {
    /// The protocol event that carries this event: projectiles and
    /// explosions have their own typed events (the 1.5 gap), every other
    /// event travels as [`encode`](Self::encode) bytes in `Event::Game`.
    #[must_use]
    pub fn to_protocol(&self) -> faset_protocol::Event {
        use faset_protocol::Event;
        match self {
            Self::Projectile(p) => Event::ProjectileSpawn(*p),
            Self::Explosion {
                owner,
                weapon,
                centre,
            } => Event::Explosion {
                owner: *owner,
                weapon: *weapon,
                centre: *centre,
            },
            other => Event::Game(other.encode()),
        }
    }

    /// Inverse of [`to_protocol`](Self::to_protocol); `None` for other
    /// protocol events or malformed bytes.
    #[must_use]
    pub fn from_protocol(event: &faset_protocol::Event) -> Option<Self> {
        use faset_protocol::Event;
        match event {
            Event::ProjectileSpawn(p) => Some(Self::Projectile(*p)),
            Event::Explosion {
                owner,
                weapon,
                centre,
            } => Some(Self::Explosion {
                owner: *owner,
                weapon: *weapon,
                centre: *centre,
            }),
            Event::Game(bytes) => Self::decode(bytes).ok(),
            _ => None,
        }
    }

    /// Wire bytes (lot 4.11): a tag byte, then the fields, little endian.
    /// Texts and lists carry a 16-bit length.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::PlayerJoined(p) => {
                out.push(G_JOINED);
                out.extend_from_slice(&p.0.to_le_bytes());
            }
            Self::PlayerLeft(p) => {
                out.push(G_LEFT);
                out.extend_from_slice(&p.0.to_le_bytes());
            }
            Self::PhaseChanged { phase, end } => {
                out.push(G_PHASE);
                out.push(phase.to_u8());
                out.push(u8::from(end.is_some()));
                out.extend_from_slice(&end.map_or(0, |t| t.0).to_le_bytes());
            }
            Self::ScoreChanged { player, score } => {
                out.push(G_SCORE);
                out.extend_from_slice(&player.0.to_le_bytes());
                out.extend_from_slice(&score.to_le_bytes());
            }
            Self::MatchEnded(r) => {
                out.push(G_ENDED);
                let n = r.winners.len().min(usize::from(u16::MAX));
                out.extend_from_slice(&(n as u16).to_le_bytes());
                for w in &r.winners[..n] {
                    out.extend_from_slice(&w.0.to_le_bytes());
                }
                out.push(r.winning_team.map_or(u8::MAX, |t| t.0));
                out.push(match r.reason {
                    EndReason::WinCondition => 0,
                    EndReason::TimeLimit => 1,
                });
            }
            Self::MapChanged { map } => {
                out.push(G_MAP);
                push_text(&mut out, map);
            }
            Self::Feed(text) => {
                out.push(G_FEED);
                push_text(&mut out, text);
            }
            Self::Mode(bytes) => {
                out.push(G_MODE);
                out.extend_from_slice(bytes);
            }
            Self::Projectile(p) => {
                out.push(G_PROJECTILE);
                out.extend_from_slice(&p.shooter.0.to_le_bytes());
                out.push(p.weapon);
                out.extend_from_slice(&p.tick.0.to_le_bytes());
                out.extend_from_slice(&p.seed.to_le_bytes());
                push_vec3(&mut out, p.origin);
                push_vec3(&mut out, p.velocity);
            }
            Self::Explosion {
                owner,
                weapon,
                centre,
            } => {
                out.push(G_EXPLOSION);
                out.extend_from_slice(&owner.0.to_le_bytes());
                out.push(*weapon);
                push_vec3(&mut out, *centre);
            }
        }
        out
    }

    /// Inverse of [`encode`](Self::encode).
    ///
    /// # Errors
    /// Truncated or trailing bytes, an unknown tag or phase, bad UTF-8 or a
    /// non-finite float.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        let tag = r.u8()?;
        let event = match tag {
            G_JOINED => Self::PlayerJoined(NetId(r.u16()?)),
            G_LEFT => Self::PlayerLeft(NetId(r.u16()?)),
            G_PHASE => {
                let code = r.u8()?;
                let phase = Phase::from_u8(code).ok_or(DecodeError::Phase(code))?;
                let timed = r.u8()? != 0;
                let end = r.u32()?;
                Self::PhaseChanged {
                    phase,
                    end: timed.then_some(Tick(end)),
                }
            }
            G_SCORE => Self::ScoreChanged {
                player: NetId(r.u16()?),
                score: r.u32()? as i32,
            },
            G_ENDED => {
                let n = r.u16()?;
                let winners = (0..n)
                    .map(|_| r.u16().map(NetId))
                    .collect::<Result<_, _>>()?;
                let winning_team = match r.u8()? {
                    u8::MAX => None,
                    t => Some(TeamId(t)),
                };
                let reason = match r.u8()? {
                    0 => EndReason::WinCondition,
                    1 => EndReason::TimeLimit,
                    _ => return Err(DecodeError::Invalid),
                };
                Self::MatchEnded(MatchResult {
                    winners,
                    winning_team,
                    reason,
                })
            }
            G_MAP => Self::MapChanged { map: r.text()? },
            G_FEED => Self::Feed(r.text()?),
            G_MODE => {
                let rest = r.0.to_vec();
                r.0 = &[];
                Self::Mode(rest)
            }
            G_PROJECTILE => Self::Projectile(ProjectileSpawn {
                shooter: NetId(r.u16()?),
                weapon: r.u8()?,
                tick: Tick(r.u32()?),
                seed: r.u32()?,
                origin: r.vec3()?,
                velocity: r.vec3()?,
            }),
            G_EXPLOSION => Self::Explosion {
                owner: NetId(r.u16()?),
                weapon: r.u8()?,
                centre: r.vec3()?,
            },
            _ => return Err(DecodeError::Invalid),
        };
        if !r.0.is_empty() {
            return Err(DecodeError::TrailingData);
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_round_trips() {
        let events = [
            GameEvent::PlayerJoined(NetId(3)),
            GameEvent::PlayerLeft(NetId(4)),
            GameEvent::PhaseChanged {
                phase: Phase::Countdown,
                end: Some(Tick(99)),
            },
            GameEvent::PhaseChanged {
                phase: Phase::Warmup,
                end: None,
            },
            GameEvent::ScoreChanged {
                player: NetId(1),
                score: -2,
            },
            GameEvent::MatchEnded(MatchResult {
                winners: vec![NetId(1), NetId(5)],
                winning_team: Some(TeamId(1)),
                reason: EndReason::TimeLimit,
            }),
            GameEvent::MatchEnded(MatchResult::winner(NetId(2))),
            GameEvent::MapChanged {
                map: "arena".into(),
            },
            GameEvent::Feed("P1 killed P2".into()),
            GameEvent::Mode(vec![1, 2, 3]),
            GameEvent::Projectile(ProjectileSpawn {
                shooter: NetId(7),
                weapon: 2,
                tick: Tick(500),
                seed: 9,
                origin: Vec3::new(1.0, 2.0, 3.0),
                velocity: Vec3::new(0.0, 0.5, -30.0),
            }),
            GameEvent::Explosion {
                owner: NetId(7),
                weapon: 2,
                centre: Vec3::new(-1.0, 0.25, 4.0),
            },
        ];
        for e in events {
            assert_eq!(GameEvent::decode(&e.encode()), Ok(e.clone()), "{e:?}");
            let mut long = e.encode();
            if !matches!(e, GameEvent::Mode(_)) {
                long.push(0);
                assert!(GameEvent::decode(&long).is_err(), "{e:?}");
                let short = &e.encode()[..e.encode().len() - 1];
                assert!(GameEvent::decode(short).is_err(), "{e:?}");
            }
        }
        assert!(GameEvent::decode(&[]).is_err());
        assert!(GameEvent::decode(&[200]).is_err());
    }
}
