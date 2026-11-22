//! The FPS HUD data model (lot 4.8).
//!
//! [`HudState`] is the client-side memory of what the HUD shows over time
//! (kill feed, hit marker, damage numbers); the host feeds it the local
//! player's data each frame through [`HudInput`]. [`HudModel`] is built from
//! both and the replicated state every frame, and holds everything the HUD
//! draws as plain data: the pixel drawing lives in the `client` crate, since
//! `mode-fps` depends only on `minigame-api` and cannot reach `ui`/egui. The
//! mode's `draw_hud` hook writes [`HudModel::text_lines`] into the
//! framework's `HudCtx`, the only drawing surface it offers.
//!
//! Gap (reported, not worked around): the replicated mode state carries no
//! match timer and no per-player score table, and `GameModeClient` receives
//! neither, so the host supplies them in [`HudInput`] until the integration
//! lot wires `Match` into the client.

use std::collections::{BTreeMap, VecDeque};

use faset_core::NetId;

use crate::damage::{HitZone, DEFAULT_HEALTH};
use crate::message::ServerMsg;
use crate::pickup::PickupKind;

/// Kill-feed lines kept.
pub const FEED_LINES: usize = 5;
/// Seconds a kill-feed line stays.
pub const FEED_SECONDS: f32 = 6.0;
/// Seconds the hit marker fades over.
pub const HIT_MARKER_SECONDS: f32 = 0.25;
/// Seconds the kill marker fades over.
pub const KILL_MARKER_SECONDS: f32 = 0.6;
/// Seconds a damage number floats.
pub const NUMBER_SECONDS: f32 = 0.8;
/// Damage numbers kept.
pub const MAX_NUMBERS: usize = 8;

/// Pixel radius of the reticle gap for a dispersion cone of standard
/// deviation `sigma` (radians): `focal_px * tan(sigma)`, with
/// `focal_px = (H / 2) / tan(fov_y / 2)`.
#[must_use]
pub fn reticle_radius_px(sigma: f32, fov_y: f32, height_px: f32) -> f32 {
    focal_px(fov_y, height_px) * sigma.tan()
}

/// Focal length in pixels of a view `height_px` tall with vertical field of
/// view `fov_y` (radians).
#[must_use]
pub fn focal_px(fov_y: f32, height_px: f32) -> f32 {
    (height_px * 0.5) / (fov_y * 0.5).tan()
}

/// One row of the scoreboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    pub name: String,
    pub score: i32,
    pub kills: u32,
    pub deaths: u32,
    /// The row of the local player.
    pub local: bool,
}

/// When the class-choice screen is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceMoment {
    RoundStart,
    Death,
}

/// One class on the class-choice screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassOption {
    pub name: String,
    /// One line: health, speed, main weapon.
    pub summary: String,
}

/// The class-choice screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassChoice {
    pub moment: ChoiceMoment,
    pub options: Vec<ClassOption>,
    pub selected: usize,
}

/// What the host knows and the mode does not, set each frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HudInput {
    /// The local player.
    pub player: Option<NetId>,
    /// Display names for the kill feed (missing: `P<id>`).
    pub names: BTreeMap<NetId, String>,
    /// Name of the weapon in hand.
    pub weapon: String,
    /// Magazine and reserve rounds of the weapon in hand.
    pub ammo: Option<(u16, u16)>,
    /// Speed, metres per second.
    pub speed: f32,
    /// Standard deviation of the current dispersion cone, radians (see
    /// [`cone_sigma`](crate::cone_sigma)).
    pub sigma: f32,
    /// Seconds left in the match.
    pub time_left: Option<f32>,
    /// Score table, best first.
    pub scores: Vec<ScoreRow>,
    /// Tab is held.
    pub show_scoreboard: bool,
    /// The match is in its `RoundEnd` phase: the scoreboard stays up.
    pub round_end: bool,
    pub class_choice: Option<ClassChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FeedEntry {
    attacker: NetId,
    victim: NetId,
    age: f32,
}

/// A number floating up from a hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageNumber {
    pub amount: f32,
    /// 0 when it appears, 1 when it is gone.
    pub age: f32,
    pub head: bool,
}

/// What the HUD remembers between frames.
#[derive(Debug, Clone, Default)]
pub struct HudState {
    input: HudInput,
    feed: VecDeque<FeedEntry>,
    hit: f32,
    kill: f32,
    numbers: Vec<(f32, f32, bool)>,
}

impl HudState {
    /// Replaces what the host supplies.
    pub fn set_input(&mut self, input: HudInput) {
        self.input = input;
    }

    /// The input of this frame.
    #[must_use]
    pub fn input(&self) -> &HudInput {
        &self.input
    }

    /// Takes a server message into account (kills and hits).
    pub fn on_msg(&mut self, msg: &ServerMsg) {
        match *msg {
            ServerMsg::Kill { attacker, victim } => {
                if self.feed.len() == FEED_LINES {
                    self.feed.pop_front();
                }
                self.feed.push_back(FeedEntry {
                    attacker,
                    victim,
                    age: 0.0,
                });
                if Some(attacker) == self.input.player && attacker != victim {
                    self.kill = KILL_MARKER_SECONDS;
                }
            }
            ServerMsg::Hit {
                attacker,
                victim,
                amount,
                zone,
            } if Some(attacker) == self.input.player && attacker != victim => {
                self.hit = HIT_MARKER_SECONDS;
                if self.numbers.len() == MAX_NUMBERS {
                    self.numbers.remove(0);
                }
                self.numbers.push((amount, 0.0, zone == HitZone::Head));
            }
            _ => {}
        }
    }

    /// Ages feed lines, markers and numbers by `dt` seconds.
    pub fn advance(&mut self, dt: f32) {
        for e in &mut self.feed {
            e.age += dt;
        }
        while self.feed.front().is_some_and(|e| e.age >= FEED_SECONDS) {
            self.feed.pop_front();
        }
        self.hit = (self.hit - dt).max(0.0);
        self.kill = (self.kill - dt).max(0.0);
        for n in &mut self.numbers {
            n.1 += dt / NUMBER_SECONDS;
        }
        self.numbers.retain(|n| n.1 < 1.0);
    }

    fn name(&self, id: NetId) -> String {
        self.input
            .names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("P{}", id.0))
    }

    /// Builds the model of this frame. `own` is the local player's replicated
    /// `(health, armour, alive)`; `bonuses` the timed bonuses with seconds
    /// left; `team_scores` the team table of team modes.
    #[must_use]
    pub fn model(
        &self,
        own: Option<(u16, u16, bool)>,
        bonuses: Vec<(PickupKind, f32)>,
        team_scores: Vec<i32>,
    ) -> HudModel {
        let i = &self.input;
        let (health, armour, alive) = own.unwrap_or((DEFAULT_HEALTH as u16, 0, true));
        HudModel {
            alive,
            health,
            max_health: DEFAULT_HEALTH as u16,
            armour,
            weapon: i.weapon.clone(),
            ammo: i.ammo,
            speed: i.speed,
            sigma: i.sigma,
            time_left: i.time_left,
            bonuses: bonuses
                .into_iter()
                .filter(|&(_, s)| s > 0.0)
                .map(|(kind, remaining)| BonusView { kind, remaining })
                .collect(),
            feed: self
                .feed
                .iter()
                .map(|e| FeedLine {
                    attacker: self.name(e.attacker),
                    victim: self.name(e.victim),
                    local: Some(e.attacker) == i.player || Some(e.victim) == i.player,
                    age: e.age,
                })
                .collect(),
            hit_marker: (self.hit / HIT_MARKER_SECONDS).clamp(0.0, 1.0),
            kill_marker: (self.kill / KILL_MARKER_SECONDS).clamp(0.0, 1.0),
            numbers: self
                .numbers
                .iter()
                .map(|&(amount, age, head)| DamageNumber { amount, age, head })
                .collect(),
            team_scores,
            leaders: i.scores.iter().take(3).cloned().collect(),
            scoreboard: (i.show_scoreboard || i.round_end).then(|| i.scores.clone()),
            class_choice: i.class_choice.clone(),
        }
    }
}

/// A timed bonus on the HUD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BonusView {
    pub kind: PickupKind,
    /// Seconds left.
    pub remaining: f32,
}

/// A kill-feed line.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedLine {
    pub attacker: String,
    pub victim: String,
    /// The local player is the attacker or the victim.
    pub local: bool,
    /// Seconds since it appeared.
    pub age: f32,
}

/// Everything the FPS HUD draws, as data.
#[derive(Debug, Clone, PartialEq)]
pub struct HudModel {
    pub alive: bool,
    pub health: u16,
    pub max_health: u16,
    pub armour: u16,
    pub weapon: String,
    /// Magazine and reserve.
    pub ammo: Option<(u16, u16)>,
    /// Metres per second.
    pub speed: f32,
    /// Dispersion cone, radians.
    pub sigma: f32,
    pub time_left: Option<f32>,
    pub bonuses: Vec<BonusView>,
    pub feed: Vec<FeedLine>,
    /// Hit marker opacity, 0..1.
    pub hit_marker: f32,
    /// Kill marker opacity, 0..1.
    pub kill_marker: f32,
    pub numbers: Vec<DamageNumber>,
    pub team_scores: Vec<i32>,
    /// `Some` while the scoreboard is shown (Tab or `RoundEnd`).
    pub scoreboard: Option<Vec<ScoreRow>>,
    /// The best three rows, always shown in a corner.
    pub leaders: Vec<ScoreRow>,
    pub class_choice: Option<ClassChoice>,
}

impl HudModel {
    /// Pixel radius of the reticle gap for a view `height_px` tall.
    #[must_use]
    pub fn reticle_radius_px(&self, fov_y: f32, height_px: f32) -> f32 {
        reticle_radius_px(self.sigma, fov_y, height_px)
    }

    /// Whether health is low enough to warn (a quarter or less, alive).
    #[must_use]
    pub fn low_health(&self) -> bool {
        self.alive && u32::from(self.health) * 4 <= u32::from(self.max_health)
    }

    /// The model as text lines, for the framework's `HudCtx`.
    #[must_use]
    pub fn text_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for f in &self.feed {
            lines.push(format!("{} killed {}", f.attacker, f.victim));
        }
        if let Some(t) = self.time_left {
            lines.push(format!(
                "Time {}:{:02}",
                (t / 60.0) as u32,
                (t % 60.0) as u32
            ));
        }
        lines.push(format!("HP {} ARM {}", self.health, self.armour));
        if let Some((mag, reserve)) = self.ammo {
            lines.push(format!("{} {mag}/{reserve}", self.weapon));
        }
        lines.push(format!("Speed {:.1} m/s", self.speed));
        for b in &self.bonuses {
            lines.push(format!("{} {:.1}s", b.kind.name(), b.remaining));
        }
        for (team, score) in self.team_scores.iter().enumerate() {
            lines.push(format!("Team {team}: {score}"));
        }
        if let Some(rows) = &self.scoreboard {
            for r in rows {
                lines.push(format!("{} {} ({}/{})", r.name, r.score, r.kills, r.deaths));
            }
        }
        if let Some(c) = &self.class_choice {
            for (i, o) in c.options.iter().enumerate() {
                let mark = if i == c.selected { ">" } else { " " };
                lines.push(format!("{mark} {}: {}", o.name, o.summary));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Projects a ray `sigma` off the view axis through a pinhole camera
    /// (the matrix form `x_ndc = f / aspect * x / -z`, `f = 1 / tan(fov / 2)`)
    /// and returns its distance from the screen centre, in pixels.
    fn projected_px(sigma: f32, fov_y: f32, w: f32, h: f32) -> f32 {
        let f = 1.0 / (fov_y * 0.5).tan();
        let (x, z) = (sigma.sin() * 10.0, -sigma.cos() * 10.0);
        f / (w / h) * x / -z * w * 0.5
    }

    #[test]
    fn reticle_radius_equals_focal_px_tan_sigma_within_half_a_pixel() {
        for (w, h) in [(1920.0, 1080.0), (1920.0, 1200.0), (1280.0, 720.0)] {
            for fov_deg in [70.0f32, 75.0, 85.0] {
                let fov = fov_deg.to_radians();
                for k in 0..=20 {
                    let sigma = k as f32 * 0.002;
                    let got = reticle_radius_px(sigma, fov, h);
                    let truth = projected_px(sigma, fov, w, h);
                    assert!((got - truth).abs() < 0.5, "{got} vs {truth} at {sigma}");
                    assert!((got - focal_px(fov, h) * sigma.tan()).abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn reticle_radius_grows_monotonically_with_sigma() {
        let fov = 75f32.to_radians();
        let mut prev = -1.0;
        for k in 0..100 {
            let r = reticle_radius_px(k as f32 * 0.001, fov, 1080.0);
            assert!(r > prev);
            prev = r;
        }
        assert_eq!(reticle_radius_px(0.0, fov, 1080.0), 0.0);
    }

    fn state_for(player: u16) -> HudState {
        let mut s = HudState::default();
        s.set_input(HudInput {
            player: Some(NetId(player)),
            ..HudInput::default()
        });
        s
    }

    #[test]
    fn own_hits_show_a_marker_and_a_number_that_fade() {
        let mut s = state_for(1);
        s.on_msg(&ServerMsg::Hit {
            attacker: NetId(1),
            victim: NetId(2),
            amount: 42.0,
            zone: HitZone::Head,
        });
        s.on_msg(&ServerMsg::Hit {
            attacker: NetId(3),
            victim: NetId(2),
            amount: 10.0,
            zone: HitZone::Body,
        });
        let m = s.model(None, vec![], vec![]);
        assert_eq!(m.hit_marker, 1.0);
        assert_eq!(m.numbers.len(), 1, "only the local player's hits");
        assert!(m.numbers[0].head && m.numbers[0].amount == 42.0);
        s.advance(0.2);
        let m = s.model(None, vec![], vec![]);
        assert!(m.hit_marker > 0.0 && m.hit_marker < 1.0);
        s.advance(1.0);
        let m = s.model(None, vec![], vec![]);
        assert_eq!(m.hit_marker, 0.0);
        assert!(m.numbers.is_empty());
    }

    #[test]
    fn feed_keeps_five_lines_and_expires() {
        let mut s = state_for(1);
        for v in 2..10 {
            s.on_msg(&ServerMsg::Kill {
                attacker: NetId(1),
                victim: NetId(v),
            });
        }
        let m = s.model(None, vec![], vec![]);
        assert_eq!(m.feed.len(), FEED_LINES);
        assert_eq!(m.feed[0].victim, "P5");
        assert!(m.feed.iter().all(|f| f.local));
        assert_eq!(m.kill_marker, 1.0);
        s.advance(FEED_SECONDS + 0.1);
        assert!(s.model(None, vec![], vec![]).feed.is_empty());
    }

    #[test]
    fn scoreboard_shows_on_tab_and_at_round_end_only() {
        let mut s = state_for(1);
        let row = ScoreRow {
            name: "a".into(),
            score: 3,
            kills: 3,
            deaths: 1,
            local: true,
        };
        let mut input = HudInput {
            player: Some(NetId(1)),
            scores: vec![row],
            ..HudInput::default()
        };
        s.set_input(input.clone());
        assert!(s.model(None, vec![], vec![]).scoreboard.is_none());
        input.show_scoreboard = true;
        s.set_input(input.clone());
        assert_eq!(s.model(None, vec![], vec![]).scoreboard.unwrap().len(), 1);
        input.show_scoreboard = false;
        input.round_end = true;
        s.set_input(input);
        assert!(s.model(None, vec![], vec![]).scoreboard.is_some());
    }

    #[test]
    fn low_health_warns_at_a_quarter() {
        let s = state_for(1);
        assert!(s.model(Some((25, 0, true)), vec![], vec![]).low_health());
        assert!(!s.model(Some((26, 0, true)), vec![], vec![]).low_health());
        assert!(!s.model(Some((0, 0, false)), vec![], vec![]).low_health());
    }

    #[test]
    fn expired_bonuses_are_dropped_from_the_model() {
        let s = state_for(1);
        let m = s.model(
            None,
            vec![(PickupKind::Speed, 4.5), (PickupKind::Damage, 0.0)],
            vec![],
        );
        assert_eq!(m.bonuses.len(), 1);
        assert!(m.text_lines().iter().any(|l| l == "speed 4.5s"));
    }
}
