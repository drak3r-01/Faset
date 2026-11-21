//! The one colour theme of every screen.

use egui::{Color32, CornerRadius, Stroke};

/// Colour tokens. Screens read these instead of hard-coding colours;
/// accessibility filters (colour-blind palettes) replace the struct later.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Window and panel background.
    pub background: Color32,
    /// Raised surfaces: panels, cards, text fields.
    pub surface: Color32,
    /// Surface under the pointer.
    pub surface_hover: Color32,
    /// Surface being pressed.
    pub surface_active: Color32,
    /// Borders and separators.
    pub border: Color32,
    /// Body text.
    pub text: Color32,
    /// Secondary text (hints, units).
    pub text_muted: Color32,
    /// Interactive accent: selection, focus, primary buttons.
    pub accent: Color32,
    /// Good state (low latency, healthy tick).
    pub success: Color32,
    /// Degraded state.
    pub warning: Color32,
    /// Bad state, errors.
    pub danger: Color32,
    /// Corner radius of surfaces, in points.
    pub radius: u8,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color32::from_rgba_unmultiplied(18, 20, 26, 235),
            surface: Color32::from_rgb(30, 34, 44),
            surface_hover: Color32::from_rgb(42, 48, 62),
            surface_active: Color32::from_rgb(54, 62, 80),
            border: Color32::from_rgb(64, 72, 92),
            text: Color32::from_rgb(230, 233, 240),
            text_muted: Color32::from_rgb(150, 158, 176),
            accent: Color32::from_rgb(96, 165, 250),
            success: Color32::from_rgb(74, 222, 128),
            warning: Color32::from_rgb(250, 204, 21),
            danger: Color32::from_rgb(248, 113, 113),
            radius: 4,
        }
    }
}

impl Theme {
    /// Sets the colours of `ctx` from the tokens.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        let radius = CornerRadius::same(self.radius);
        v.override_text_color = Some(self.text);
        v.window_fill = self.background;
        v.panel_fill = self.background;
        v.extreme_bg_color = self.surface;
        v.faint_bg_color = self.surface;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = radius;
        v.hyperlink_color = self.accent;
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.danger;
        v.selection.bg_fill = self.accent.gamma_multiply(0.4);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        w.noninteractive.corner_radius = radius;
        w.inactive.bg_fill = self.surface;
        w.inactive.weak_bg_fill = self.surface;
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.inactive.corner_radius = radius;
        w.hovered.bg_fill = self.surface_hover;
        w.hovered.weak_bg_fill = self.surface_hover;
        w.hovered.fg_stroke = Stroke::new(1.5, self.text);
        w.hovered.corner_radius = radius;
        w.active.bg_fill = self.surface_active;
        w.active.weak_bg_fill = self.surface_active;
        w.active.fg_stroke = Stroke::new(2.0, self.text);
        w.active.corner_radius = radius;
        ctx.set_visuals(v);
    }
}
