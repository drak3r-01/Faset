//! Path follower: turns the navigation path of lot 4.9 into a wish direction
//! and the jump and crouch buttons, using the movement flags of the level.

use faset_core::glam::Vec3;

use crate::graph::{EdgeKind, Path};
use crate::level::MoveFlags;
use crate::{BotView, DEFAULT_VMAX};

/// A goal that moved less than this keeps the current plan, metres.
const GOAL_HYSTERESIS: f32 = 2.0;
/// Shortest delay between two plans, seconds.
const MIN_PLAN_INTERVAL: f32 = 0.25;
/// Delay after which the plan is refreshed anyway (lot 4.9 re-plan period).
const PLAN_INTERVAL: f32 = crate::REPLAN_INTERVAL;
/// A waypoint is reached within this horizontal distance, metres.
const REACH: f32 = 0.55;
/// The goal is reached within this horizontal distance, metres.
const ARRIVE: f32 = 1.2;
/// Straight run needed before bunny hops and slides, metres.
const STRAIGHT_RUN: f32 = 6.0;
/// Speed needed to start a slide (above the controller's minimum), m/s.
const SLIDE_SPEED: f32 = 5.5;
/// Slide duration used by the bot, seconds; slide-hop leaves it earlier.
const SLIDE_TIME: f64 = 0.8;
const SLIDE_HOP_TIME: f64 = 0.45;
/// Movement tested for being stuck every second.
const STUCK_PERIOD: f64 = 1.0;
const STUCK_DISTANCE: f32 = 0.4;

/// What the follower asks of the controller this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Drive {
    /// Horizontal unit wish direction, or zero.
    pub dir: Vec3,
    pub jump: bool,
    pub crouch: bool,
    /// The goal is reached.
    pub arrived: bool,
    /// The bot has not moved for a while: pick another goal.
    pub stuck: bool,
    /// No path to the goal exists.
    pub unreachable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Follower {
    goal: Option<Vec3>,
    path: Option<Path>,
    idx: usize,
    since_plan: f32,
    goal_changed: bool,
    /// Ticks to wait before the first plan (spreads A* over 16 bots).
    stagger: u32,
    jump_until: f64,
    slide_start: Option<f64>,
    slide_ready_at: f64,
    stuck_ref: Vec3,
    stuck_since: f64,
    stuck_count: u32,
}

fn horizontal(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

impl Follower {
    pub fn new(stagger: u32) -> Self {
        Self {
            goal: None,
            path: None,
            idx: 1,
            since_plan: 0.0,
            goal_changed: false,
            stagger,
            jump_until: f64::NEG_INFINITY,
            slide_start: None,
            slide_ready_at: 0.0,
            stuck_ref: Vec3::ZERO,
            stuck_since: 0.0,
            stuck_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.goal = None;
        self.path = None;
        self.idx = 1;
        self.since_plan = 0.0;
        self.slide_start = None;
        self.stuck_count = 0;
    }

    fn plan(&mut self, view: &BotView, goal: Vec3) {
        self.path = view.nav.find_path(view.me.pos, goal, DEFAULT_VMAX);
        self.idx = 1;
        self.since_plan = 0.0;
        self.goal_changed = false;
    }

    /// Follows the path towards `goal`. `calm` allows bunny hops and slides
    /// (nobody is shooting at the bot and it does not shoot).
    pub fn drive(
        &mut self,
        view: &BotView,
        goal: Vec3,
        flags: MoveFlags,
        calm: bool,
        dt: f32,
    ) -> Drive {
        let pos = view.me.pos;
        let now = view.time;
        let mut out = Drive::default();

        if self.goal.is_none_or(|g| g.distance(goal) > GOAL_HYSTERESIS) {
            self.goal = Some(goal);
            self.goal_changed = true;
        }
        let goal = self.goal.unwrap_or(goal);

        self.since_plan += dt;
        if self.stagger > 0 {
            self.stagger -= 1;
        } else if (self.goal_changed && self.since_plan >= MIN_PLAN_INTERVAL)
            || self.since_plan >= PLAN_INTERVAL
            || (self.path.is_none() && self.since_plan >= MIN_PLAN_INTERVAL)
        {
            self.plan(view, goal);
        }
        out.unreachable = self.path.is_none() && self.since_plan == 0.0 && self.stagger == 0;

        // Waypoint to head to.
        let mut target = goal;
        let mut kind = EdgeKind::Walk;
        let straight;
        if let Some(path) = &self.path {
            let n = path.waypoints.len();
            while self.idx + 1 < n {
                let w = path.waypoints[self.idx];
                let close =
                    horizontal(w.pos - pos).length() < REACH && (w.pos.y - pos.y).abs() < 1.0;
                if !close {
                    break;
                }
                self.idx += 1;
            }
            let i = self.idx.min(n - 1);
            let w = path.waypoints[i];
            target = w.pos;
            kind = w.kind;
            // A jump edge starts when the bot stands on the previous node.
            if kind == EdgeKind::Jump
                && i > 0
                && horizontal(path.waypoints[i - 1].pos - pos).length() < 0.9
                && view.me.on_ground
            {
                self.jump_until = now + 0.25;
            }
            straight = Self::straight_run(path, i, pos);
        } else {
            straight = horizontal(goal - pos).length();
        }

        let to = horizontal(target - pos);
        out.dir = to.normalize_or_zero();
        out.arrived = horizontal(goal - pos).length() < ARRIVE && (goal.y - pos.y).abs() < 1.5;
        if out.arrived {
            out.dir = Vec3::ZERO;
        }

        let speed = horizontal(view.me.vel).length();
        let flat = (target.y - pos.y).abs() < 0.5;
        let straight_ok = calm
            && kind == EdgeKind::Walk
            && flat
            && straight > STRAIGHT_RUN
            && view.me.on_ground
            && !out.arrived;

        // Slide and slide-hop.
        if let Some(start) = self.slide_start {
            let hold = if flags.slide_hop {
                SLIDE_HOP_TIME
            } else {
                SLIDE_TIME
            };
            if now - start < hold && calm {
                out.crouch = true;
            } else {
                if flags.slide_hop && calm {
                    out.jump = true;
                }
                self.slide_start = None;
                self.slide_ready_at = now + 1.4;
            }
        } else if flags.slide && straight_ok && speed >= SLIDE_SPEED && now >= self.slide_ready_at {
            self.slide_start = Some(now);
            out.crouch = true;
        } else if flags.bunny_hop && straight_ok && speed >= 3.0 {
            out.jump = true;
        }

        if now < self.jump_until {
            out.jump = true;
        }

        // Stuck detection.
        if now - self.stuck_since >= STUCK_PERIOD {
            let moved = horizontal(pos - self.stuck_ref).length();
            if moved < STUCK_DISTANCE && !out.arrived && out.dir != Vec3::ZERO {
                self.stuck_count += 1;
                self.jump_until = now + 0.3;
                if self.stuck_count >= 2 {
                    out.stuck = true;
                    self.stuck_count = 0;
                    self.path = None;
                    self.goal = None;
                }
            } else {
                self.stuck_count = 0;
            }
            self.stuck_ref = pos;
            self.stuck_since = now;
        }
        out
    }

    /// Length of the straight stretch ahead of the bot: the distance to the
    /// waypoint plus the following walk segments that keep the heading
    /// within 15 degrees.
    fn straight_run(path: &Path, i: usize, pos: Vec3) -> f32 {
        let wps = &path.waypoints;
        let mut len = horizontal(wps[i].pos - pos).length();
        let mut heading = horizontal(wps[i].pos - pos).normalize_or_zero();
        let mut j = i;
        while j + 1 < wps.len() && wps[j + 1].kind == EdgeKind::Walk && len < 40.0 {
            let seg = horizontal(wps[j + 1].pos - wps[j].pos);
            let dir = seg.normalize_or_zero();
            if heading.dot(dir) < 0.966 || (wps[j + 1].pos.y - wps[j].pos.y).abs() > 0.3 {
                break;
            }
            len += seg.length();
            heading = dir;
            j += 1;
        }
        len
    }
}
