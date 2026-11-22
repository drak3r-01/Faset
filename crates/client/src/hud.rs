//! Draws the FPS HUD with egui (lot 4.8): the HUD binding of the client.
//!
//! `faset-mode-fps` builds a [`HudModel`] each frame and cannot reach egui
//! (its only dependency on the framework is `minigame-api`), so the pixel
//! drawing lives here: [`FpsHud`] is a [`Screen`] the client's [`Ui`]
//! (`faset-ui`) runs between the upscaler and the present, at display
//! resolution. Sizes scale with the height of the screen so the layout holds
//! at 16:9 and 16:10.
//!
//! [`Ui`]: faset_ui::Ui

use faset_mode_fps::hud::{ChoiceMoment, ClassChoice, ScoreRow};
use faset_mode_fps::{HudModel, PickupKind};
use faset_ui::egui::{
    self, Align2, Color32, FontId, LayerId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
};
use faset_ui::{Screen, Theme};

/// Height in points the layout constants are designed for.
const DESIGN_HEIGHT: f32 = 720.0;

/// The FPS HUD of one frame, as a UI screen.
#[derive(Debug, Clone)]
pub struct FpsHud {
    /// What to draw.
    pub model: HudModel,
    /// Vertical field of view of the 3D view, radians (reticle size).
    pub fov_y: f32,
}

impl FpsHud {
    /// A HUD for `model` seen through a view of vertical field of view
    /// `fov_y`.
    #[must_use]
    pub fn new(model: HudModel, fov_y: f32) -> Self {
        Self { model, fov_y }
    }
}

impl Screen for FpsHud {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        let painter = ctx.layer_painter(LayerId::background());
        let rect = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let k = (rect.height() / DESIGN_HEIGHT).clamp(0.4, 4.0);
        let hud = Hud {
            p: &painter,
            theme,
            rect,
            k,
        };
        let m = &self.model;
        if m.low_health() {
            hud.low_health_frame();
        }
        if m.alive {
            hud.reticle(
                m,
                m.reticle_radius_px(self.fov_y, rect.height() * ppp) / ppp,
            );
        }
        hud.markers(m);
        hud.numbers(m);
        hud.vitals(m);
        hud.bonuses(m);
        hud.ammo(m);
        hud.top(m);
        hud.feed(m);
        hud.speed(m);
        if let Some(rows) = &m.scoreboard {
            hud.scoreboard(rows);
        }
        if let Some(choice) = &m.class_choice {
            hud.class_choice(choice);
        }
    }
}

struct Hud<'a> {
    p: &'a Painter,
    theme: &'a Theme,
    rect: Rect,
    k: f32,
}

impl Hud<'_> {
    fn font(&self, size: f32) -> FontId {
        FontId::proportional(size * self.k)
    }

    /// Text with a one-pixel drop shadow so it reads over any scene.
    fn text(&self, pos: Pos2, anchor: Align2, s: &str, size: f32, colour: Color32) -> Rect {
        let font = self.font(size);
        let shadow = Color32::from_black_alpha(colour.a() / 2);
        self.p
            .text(pos + Vec2::splat(self.k), anchor, s, font.clone(), shadow);
        self.p.text(pos, anchor, s, font, colour)
    }

    fn panel(&self, rect: Rect) {
        self.p
            .rect_filled(rect, self.theme.radius, self.theme.background);
        self.p.rect_stroke(
            rect,
            self.theme.radius,
            Stroke::new(self.k, self.theme.border),
            StrokeKind::Inside,
        );
    }

    fn margin(&self) -> f32 {
        24.0 * self.k
    }

    /// Dynamic reticle: four ticks whose gap is the dispersion cone.
    fn reticle(&self, m: &HudModel, radius_pts: f32) {
        let c = self.rect.center();
        let gap = radius_pts.max(2.0 * self.k);
        let len = 9.0 * self.k;
        let colour = if m.hit_marker > 0.0 {
            self.theme.text
        } else {
            Color32::from_white_alpha(230)
        };
        for dir in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            let (a, b) = (c + dir * gap, c + dir * (gap + len));
            self.p.line_segment(
                [a, b],
                Stroke::new(3.0 * self.k, Color32::from_black_alpha(140)),
            );
            self.p
                .line_segment([a, b], Stroke::new(1.5 * self.k, colour));
        }
        self.p.circle_filled(c, 1.2 * self.k, colour);
    }

    /// Hit marker (white X) and kill marker (red X).
    fn markers(&self, m: &HudModel) {
        let c = self.rect.center();
        let cross = |reach: f32, alpha: f32, colour: Color32, width: f32| {
            let colour = colour.gamma_multiply(alpha);
            for sign in [Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)] {
                for flip in [1.0, -1.0] {
                    let d = sign * flip;
                    let near = c + d * (reach * 0.55);
                    let far = c + d * reach;
                    self.p
                        .line_segment([near, far], Stroke::new(width * self.k, colour));
                }
            }
        };
        if m.hit_marker > 0.0 {
            cross(14.0 * self.k, m.hit_marker, Color32::WHITE, 2.0);
        }
        if m.kill_marker > 0.0 {
            cross(20.0 * self.k, m.kill_marker, self.theme.danger, 3.0);
        }
    }

    /// Damage numbers floating up from the reticle.
    fn numbers(&self, m: &HudModel) {
        let c = self.rect.center();
        for (i, n) in m.numbers.iter().enumerate() {
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            let pos = c + Vec2::new(
                side * (22.0 + 6.0 * (i % 3) as f32) * self.k,
                (-26.0 - 46.0 * n.age) * self.k,
            );
            let base = if n.head {
                self.theme.warning
            } else {
                self.theme.text
            };
            let alpha = (1.0 - n.age).clamp(0.0, 1.0);
            let size = if n.head { 24.0 } else { 19.0 };
            self.text(
                pos,
                Align2::CENTER_CENTER,
                &format!("{:.0}", n.amount),
                size,
                base.gamma_multiply(alpha),
            );
        }
    }

    /// Health and armour bars, bottom left.
    fn vitals(&self, m: &HudModel) {
        let (w, h) = (230.0 * self.k, 16.0 * self.k);
        let x = self.rect.left() + self.margin();
        let y = self.rect.bottom() - self.margin() - h;
        let bar = |y: f32, frac: f32, colour: Color32, label: &str, value: u16| {
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
            self.p.rect_filled(r, 2.0, Color32::from_black_alpha(150));
            let fill = Rect::from_min_size(r.min, Vec2::new(w * frac.clamp(0.0, 1.0), h));
            self.p.rect_filled(fill, 2.0, colour);
            self.p.rect_stroke(
                r,
                2.0,
                Stroke::new(self.k, self.theme.border),
                StrokeKind::Inside,
            );
            self.text(
                Pos2::new(x + 6.0 * self.k, r.center().y),
                Align2::LEFT_CENTER,
                label,
                11.0,
                self.theme.text_muted,
            );
            self.text(
                Pos2::new(r.right() - 6.0 * self.k, r.center().y),
                Align2::RIGHT_CENTER,
                &value.to_string(),
                14.0,
                self.theme.text,
            );
        };
        let health_frac = f32::from(m.health) / f32::from(m.max_health.max(1));
        let health_colour = if m.low_health() {
            self.theme.danger
        } else {
            self.theme.success
        };
        bar(y, health_frac, health_colour, "HP", m.health);
        if m.armour > 0 {
            bar(
                y - h - 5.0 * self.k,
                f32::from(m.armour) / 100.0,
                self.theme.accent,
                "ARM",
                m.armour,
            );
        }
    }

    /// Red frame around the screen at low health.
    fn low_health_frame(&self) {
        let edge = 26.0 * self.k;
        let red = Color32::from_rgba_unmultiplied(220, 30, 30, 70);
        let r = self.rect;
        for strip in [
            Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + edge)),
            Rect::from_min_max(Pos2::new(r.left(), r.bottom() - edge), r.max),
            Rect::from_min_max(
                Pos2::new(r.left(), r.top() + edge),
                Pos2::new(r.left() + edge, r.bottom() - edge),
            ),
            Rect::from_min_max(
                Pos2::new(r.right() - edge, r.top() + edge),
                Pos2::new(r.right(), r.bottom() - edge),
            ),
        ] {
            self.p.rect_filled(strip, 0.0, red);
        }
    }

    /// Active timed bonuses with their remaining time, above the bars.
    fn bonuses(&self, m: &HudModel) {
        let x = self.rect.left() + self.margin();
        let mut y = self.rect.bottom() - self.margin() - 16.0 * self.k - 5.0 * self.k;
        if m.armour > 0 {
            y -= 21.0 * self.k;
        }
        for b in &m.bonuses {
            y -= 24.0 * self.k;
            let colour = match b.kind {
                PickupKind::Speed => self.theme.accent,
                PickupKind::Damage => self.theme.danger,
                PickupKind::SuperJump => self.theme.success,
                _ => self.theme.warning,
            };
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(150.0 * self.k, 20.0 * self.k));
            self.p.rect_filled(r, 2.0, Color32::from_black_alpha(150));
            self.p.rect_filled(
                Rect::from_min_size(r.min, Vec2::new(4.0 * self.k, r.height())),
                0.0,
                colour,
            );
            self.text(
                Pos2::new(x + 10.0 * self.k, r.center().y),
                Align2::LEFT_CENTER,
                &b.kind.name().to_uppercase(),
                12.0,
                self.theme.text,
            );
            self.text(
                Pos2::new(r.right() - 6.0 * self.k, r.center().y),
                Align2::RIGHT_CENTER,
                &format!("{:.1}s", b.remaining),
                12.0,
                colour,
            );
        }
    }

    /// Magazine and reserve, bottom right.
    fn ammo(&self, m: &HudModel) {
        let Some((mag, reserve)) = m.ammo else {
            return;
        };
        let right = self.rect.right() - self.margin();
        let base = self.rect.bottom() - self.margin();
        let low = mag <= 3;
        let colour = if low {
            self.theme.danger
        } else {
            self.theme.text
        };
        let reserve_box = self.text(
            Pos2::new(right, base),
            Align2::RIGHT_BOTTOM,
            &format!("/ {reserve}"),
            20.0,
            self.theme.text_muted,
        );
        let mag_box = self.text(
            Pos2::new(reserve_box.left() - 6.0 * self.k, base + 4.0 * self.k),
            Align2::RIGHT_BOTTOM,
            &mag.to_string(),
            40.0,
            colour,
        );
        self.text(
            Pos2::new(right, mag_box.top() - 2.0 * self.k),
            Align2::RIGHT_BOTTOM,
            &m.weapon.to_uppercase(),
            12.0,
            self.theme.text_muted,
        );
    }

    /// Match timer in the middle, scores on both sides.
    fn top(&self, m: &HudModel) {
        let top = self.rect.top() + 14.0 * self.k;
        let cx = self.rect.center().x;
        if let Some(t) = m.time_left {
            let secs = t.max(0.0) as u32;
            let r = Rect::from_center_size(
                Pos2::new(cx, top + 14.0 * self.k),
                Vec2::new(84.0 * self.k, 28.0 * self.k),
            );
            self.panel(r);
            let colour = if secs < 30 {
                self.theme.warning
            } else {
                self.theme.text
            };
            self.text(
                r.center(),
                Align2::CENTER_CENTER,
                &format!("{}:{:02}", secs / 60, secs % 60),
                20.0,
                colour,
            );
        }
        if m.team_scores.len() >= 2 {
            let sides = [(-1.0, self.theme.accent), (1.0, self.theme.danger)];
            for (team, (side, colour)) in sides.into_iter().enumerate() {
                let r = Rect::from_center_size(
                    Pos2::new(cx + side * 70.0 * self.k, top + 14.0 * self.k),
                    Vec2::new(48.0 * self.k, 28.0 * self.k),
                );
                self.p
                    .rect_filled(r, self.theme.radius, colour.gamma_multiply(0.55));
                self.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    &m.team_scores[team].to_string(),
                    20.0,
                    self.theme.text,
                );
            }
        } else {
            let mut y = top;
            for (i, row) in m.leaders.iter().enumerate() {
                let colour = if row.local {
                    self.theme.accent
                } else {
                    self.theme.text
                };
                self.text(
                    Pos2::new(self.rect.left() + self.margin(), y),
                    Align2::LEFT_TOP,
                    &format!("{}  {}  {}", i + 1, row.name, row.score),
                    14.0,
                    colour,
                );
                y += 19.0 * self.k;
            }
        }
    }

    /// Kill feed, top right, newest at the bottom.
    fn feed(&self, m: &HudModel) {
        let right = self.rect.right() - self.margin();
        let mut y = self.rect.top() + 14.0 * self.k;
        for line in &m.feed {
            let fade = (1.0 - (line.age - 5.0).max(0.0)).clamp(0.0, 1.0);
            let text = format!("{}  >  {}", line.attacker, line.victim);
            let galley = self
                .p
                .layout_no_wrap(text.clone(), self.font(14.0), Color32::WHITE);
            let size = galley.size() + Vec2::new(16.0, 6.0) * self.k;
            let r = Rect::from_min_size(Pos2::new(right - size.x, y), size);
            self.p
                .rect_filled(r, 2.0, Color32::from_black_alpha(150).gamma_multiply(fade));
            if line.local {
                self.p.rect_stroke(
                    r,
                    2.0,
                    Stroke::new(1.5 * self.k, self.theme.accent.gamma_multiply(fade)),
                    StrokeKind::Inside,
                );
            }
            self.text(
                r.center(),
                Align2::CENTER_CENTER,
                &text,
                14.0,
                self.theme.text.gamma_multiply(fade),
            );
            y += size.y + 4.0 * self.k;
        }
    }

    /// Speed readout under the reticle.
    fn speed(&self, m: &HudModel) {
        self.text(
            Pos2::new(self.rect.center().x, self.rect.bottom() - self.margin()),
            Align2::CENTER_BOTTOM,
            &format!("{:.0} m/s", m.speed),
            16.0,
            self.theme.text_muted,
        );
    }

    /// The scoreboard (Tab and round end).
    fn scoreboard(&self, rows: &[ScoreRow]) {
        let row_h = 24.0 * self.k;
        let w = 420.0 * self.k;
        let h = row_h * (rows.len() as f32 + 2.0) + 16.0 * self.k;
        let r = Rect::from_center_size(self.rect.center(), Vec2::new(w, h));
        self.panel(r);
        let x0 = r.left() + 16.0 * self.k;
        let cols = [
            r.right() - 150.0 * self.k,
            r.right() - 90.0 * self.k,
            r.right() - 30.0 * self.k,
        ];
        let mut y = r.top() + 8.0 * self.k + row_h * 0.5;
        self.text(
            Pos2::new(x0, y),
            Align2::LEFT_CENTER,
            "SCOREBOARD",
            14.0,
            self.theme.text_muted,
        );
        for (x, label) in cols.into_iter().zip(["SCORE", "K", "D"]) {
            self.text(
                Pos2::new(x, y),
                Align2::CENTER_CENTER,
                label,
                12.0,
                self.theme.text_muted,
            );
        }
        y += row_h;
        self.p.hline(
            r.x_range().shrink(10.0 * self.k),
            y - row_h * 0.5,
            Stroke::new(self.k, self.theme.border),
        );
        for row in rows {
            if row.local {
                self.p.rect_filled(
                    Rect::from_center_size(
                        Pos2::new(r.center().x, y),
                        Vec2::new(w - 12.0 * self.k, row_h - 2.0 * self.k),
                    ),
                    2.0,
                    self.theme.accent.gamma_multiply(0.3),
                );
            }
            self.text(
                Pos2::new(x0, y),
                Align2::LEFT_CENTER,
                &row.name,
                15.0,
                self.theme.text,
            );
            for (x, v) in cols.into_iter().zip([
                row.score.to_string(),
                row.kills.to_string(),
                row.deaths.to_string(),
            ]) {
                self.text(
                    Pos2::new(x, y),
                    Align2::CENTER_CENTER,
                    &v,
                    15.0,
                    self.theme.text,
                );
            }
            y += row_h;
        }
    }

    /// The class-choice screen at round start and at death.
    fn class_choice(&self, choice: &ClassChoice) {
        let n = choice.options.len().max(1) as f32;
        let card = Vec2::new(150.0 * self.k, 150.0 * self.k);
        let gap = 12.0 * self.k;
        let w = n * card.x + (n - 1.0) * gap + 40.0 * self.k;
        let h = card.y + 90.0 * self.k;
        let r = Rect::from_center_size(self.rect.center(), Vec2::new(w, h));
        self.panel(r);
        let title = match choice.moment {
            ChoiceMoment::RoundStart => "CHOOSE YOUR CLASS",
            ChoiceMoment::Death => "YOU DIED - CHOOSE YOUR CLASS",
        };
        self.text(
            Pos2::new(r.center().x, r.top() + 24.0 * self.k),
            Align2::CENTER_CENTER,
            title,
            18.0,
            self.theme.text,
        );
        for (i, option) in choice.options.iter().enumerate() {
            let min = Pos2::new(
                r.left() + 20.0 * self.k + i as f32 * (card.x + gap),
                r.top() + 50.0 * self.k,
            );
            let c = Rect::from_min_size(min, card);
            let selected = i == choice.selected;
            self.p.rect_filled(
                c,
                self.theme.radius,
                if selected {
                    self.theme.surface_active
                } else {
                    self.theme.surface
                },
            );
            self.p.rect_stroke(
                c,
                self.theme.radius,
                Stroke::new(
                    if selected { 2.5 } else { 1.0 } * self.k,
                    if selected {
                        self.theme.accent
                    } else {
                        self.theme.border
                    },
                ),
                StrokeKind::Inside,
            );
            self.text(
                Pos2::new(c.center().x, c.top() + 20.0 * self.k),
                Align2::CENTER_CENTER,
                &format!("{} {}", i + 1, option.name),
                16.0,
                self.theme.text,
            );
            let galley = self.p.layout(
                option.summary.clone(),
                self.font(12.0),
                self.theme.text_muted,
                card.x - 16.0 * self.k,
            );
            self.p.galley(
                Pos2::new(c.left() + 8.0 * self.k, c.top() + 42.0 * self.k),
                galley,
                self.theme.text_muted,
            );
        }
    }
}
