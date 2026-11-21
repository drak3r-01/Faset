//! The debug overlay: frame times, network graph, simulation statistics and
//! the `puffin` profiler. Hidden by default; the toggle key comes from the
//! configuration ([`crate::UiConfig::overlay_key`]).

use std::collections::VecDeque;

use egui::{Align2, Color32, Pos2, Sense, Shape, Stroke, Vec2};

use crate::i18n::text;
use crate::theme::Theme;

/// Samples kept for each graph.
const HISTORY: usize = 120;
/// Height of a graph, in points.
const GRAPH_HEIGHT: f32 = 44.0;

/// One network measurement, pushed by the client every frame or every packet
/// batch.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NetSample {
    /// Round-trip time in milliseconds.
    pub rtt_ms: f32,
    /// Received bandwidth in kilobits per second.
    pub in_kbps: f32,
    /// Sent bandwidth in kilobits per second.
    pub out_kbps: f32,
    /// Packet loss, 0 to 1.
    pub loss: f32,
}

/// Simulation statistics shown by the overlay.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SimStats {
    pub tick: u64,
    /// Time of the last simulation tick in milliseconds.
    pub tick_ms: f32,
    pub entities: u32,
    pub players: u32,
}

/// What the UI itself costs, filled by [`crate::Ui`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UiStats {
    /// CPU time of the last [`crate::Ui::draw`], in milliseconds.
    pub cpu_ms: f32,
    /// Draw calls of the last UI pass.
    pub draw_calls: u32,
    /// Vertices of the last UI pass.
    pub vertices: u32,
}

/// State of the debug overlay.
#[derive(Debug, Default)]
pub struct Overlay {
    visible: bool,
    frame_ms: VecDeque<f32>,
    network: VecDeque<NetSample>,
    sim: Option<SimStats>,
    profiler_ms: Option<f32>,
}

impl Overlay {
    /// Shows the overlay if hidden, hides it if shown. Also turns the
    /// profiler's scopes on or off: they cost nothing while hidden.
    pub fn toggle(&mut self) {
        self.set_visible(!self.visible);
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        puffin::set_scopes_on(visible);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Records the duration of the previous frame, in seconds.
    pub fn record_frame_time(&mut self, seconds: f32) {
        push(&mut self.frame_ms, seconds * 1000.0);
    }

    /// Records a network measurement.
    pub fn record_network(&mut self, sample: NetSample) {
        push(&mut self.network, sample);
    }

    /// Sets the simulation statistics (`None`: no simulation is running).
    pub fn set_sim_stats(&mut self, stats: Option<SimStats>) {
        self.sim = stats;
    }

    pub(crate) fn set_profiler_frame_ms(&mut self, ms: Option<f32>) {
        self.profiler_ms = ms;
    }

    /// Draws the overlay if it is visible.
    pub(crate) fn show(&self, ctx: &egui::Context, theme: &Theme, ui_stats: UiStats) {
        if !self.visible {
            return;
        }
        egui::Window::new(text("overlay.title"))
            .anchor(Align2::LEFT_TOP, Vec2::splat(8.0))
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .interactable(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                self.frame_section(ui, theme);
                ui.separator();
                self.network_section(ui, theme);
                ui.separator();
                self.sim_section(ui, theme);
                ui.separator();
                self.profiler_section(ui, theme);
                ui.separator();
                ui.heading(text("overlay.ui"));
                ui.label(format!(
                    "{} {:.2} ms | {} {} | {} {}",
                    text("overlay.ui.cpu"),
                    ui_stats.cpu_ms,
                    text("overlay.ui.draw_calls"),
                    ui_stats.draw_calls,
                    text("overlay.ui.vertices"),
                    ui_stats.vertices
                ));
            });
    }

    fn frame_section(&self, ui: &mut egui::Ui, theme: &Theme) {
        ui.heading(text("overlay.frame_times"));
        let last = self.frame_ms.back().copied().unwrap_or(0.0);
        let avg = mean(self.frame_ms.iter().copied());
        let max = self.frame_ms.iter().copied().fold(0.0, f32::max);
        let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
        ui.label(format!(
            "{} {last:.1} ms | {} {avg:.1} | {} {max:.1} | {fps:.0} {}",
            text("overlay.frame_ms"),
            text("overlay.frame_avg"),
            text("overlay.frame_max"),
            text("overlay.fps"),
        ));
        // 16.7 ms (60 FPS) is the budget line; the graph scale never drops
        // below 33 ms so a steady frame does not fill it.
        let scale = max.max(33.3);
        let colour = if last <= 16.7 {
            theme.success
        } else if last <= 33.3 {
            theme.warning
        } else {
            theme.danger
        };
        graph(
            ui,
            theme,
            &self.frame_ms.iter().copied().collect::<Vec<_>>(),
            colour,
            scale,
            Some(16.7),
        );
    }

    fn network_section(&self, ui: &mut egui::Ui, theme: &Theme) {
        ui.heading(text("overlay.network"));
        let Some(last) = self.network.back() else {
            ui.colored_label(theme.text_muted, text("overlay.network.none"));
            return;
        };
        ui.label(format!(
            "{} {:.0} ms | {} {:.1} kbps | {} {:.1} kbps | {} {:.1} %",
            text("overlay.network.rtt"),
            last.rtt_ms,
            text("overlay.network.in"),
            last.in_kbps,
            text("overlay.network.out"),
            last.out_kbps,
            text("overlay.network.loss"),
            last.loss * 100.0,
        ));
        let rtt: Vec<f32> = self.network.iter().map(|s| s.rtt_ms).collect();
        let scale = rtt.iter().copied().fold(0.0, f32::max).max(100.0);
        graph(ui, theme, &rtt, theme.accent, scale, None);
    }

    fn sim_section(&self, ui: &mut egui::Ui, theme: &Theme) {
        ui.heading(text("overlay.simulation"));
        match &self.sim {
            Some(s) => {
                ui.label(format!(
                    "{} {} | {} {:.2} ms | {} {} | {} {}",
                    text("overlay.simulation.tick"),
                    s.tick,
                    text("overlay.simulation.tick_ms"),
                    s.tick_ms,
                    text("overlay.simulation.entities"),
                    s.entities,
                    text("overlay.simulation.players"),
                    s.players,
                ));
            }
            None => {
                ui.colored_label(theme.text_muted, text("overlay.simulation.none"));
            }
        }
    }

    fn profiler_section(&self, ui: &mut egui::Ui, theme: &Theme) {
        ui.heading(text("overlay.profiler"));
        match self.profiler_ms {
            Some(ms) => {
                ui.label(format!("{} {ms:.2} ms", text("overlay.profiler.frame")));
            }
            None => {
                ui.colored_label(theme.text_muted, text("overlay.profiler.off"));
            }
        }
    }
}

fn push<T>(queue: &mut VecDeque<T>, value: T) {
    if queue.len() == HISTORY {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn mean(values: impl Iterator<Item = f32>) -> f32 {
    let (sum, n) = values.fold((0.0, 0u32), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// A line graph of `values` on a 0..`scale` axis, with an optional
/// horizontal guide.
fn graph(
    ui: &mut egui::Ui,
    theme: &Theme,
    values: &[f32],
    colour: Color32,
    scale: f32,
    guide: Option<f32>,
) {
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, GRAPH_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme.surface);
    let y = |v: f32| rect.bottom() - (v / scale).clamp(0.0, 1.0) * rect.height();
    if let Some(g) = guide {
        painter.hline(rect.x_range(), y(g), Stroke::new(1.0, theme.border));
    }
    let step = rect.width() / (HISTORY - 1) as f32;
    let offset = (HISTORY - values.len().min(HISTORY)) as f32 * step;
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| Pos2::new(rect.left() + offset + i as f32 * step, y(*v)))
        .collect();
    if points.len() > 1 {
        painter.add(Shape::line(points, Stroke::new(1.5, colour)));
    }
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme.border),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded() {
        let mut o = Overlay::default();
        for i in 0..HISTORY * 2 {
            o.record_frame_time(i as f32);
        }
        assert_eq!(o.frame_ms.len(), HISTORY);
        assert_eq!(*o.frame_ms.front().unwrap(), HISTORY as f32 * 1000.0);
    }

    #[test]
    fn toggle_flips_visibility() {
        let mut o = Overlay::default();
        assert!(!o.is_visible());
        o.toggle();
        assert!(o.is_visible());
        o.toggle();
        assert!(!o.is_visible());
    }
}
