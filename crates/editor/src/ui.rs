//! egui toolbar and viewport input of the editor.

use faset_core::glam::{Vec2, Vec3};
use faset_ui::egui;
use faset_ui::{Screen, Theme};

use crate::camera::{CameraInput, CameraMode};
use crate::create::BrushKind;
use crate::editor::{Editor, Tool};
use crate::handles::HandleMode;
use crate::sculpt::SculptOp;

/// What the viewport saw this frame, gathered from egui before acting.
struct Frame {
    dt: f32,
    camera: CameraInput,
    pointer: Option<Vec2>,
    pressed: bool,
    released: bool,
    undo: bool,
    redo: bool,
    delete: bool,
    cancel: bool,
    toggle_camera: bool,
    mode: Option<HandleMode>,
}

fn gather(ctx: &egui::Context) -> Frame {
    let typing = ctx.egui_wants_keyboard_input();
    let over_ui = ctx.egui_wants_pointer_input();
    ctx.input(|i| {
        let key = |k: egui::Key| !typing && i.key_down(k);
        let pressed = |k: egui::Key| !typing && i.key_pressed(k);
        let axis = |pos: egui::Key, neg: egui::Key| {
            f32::from(u8::from(key(pos))) - f32::from(u8::from(key(neg)))
        };
        let ctrl = i.modifiers.command;
        let looking = i.pointer.secondary_down();
        let delta = i.pointer.delta();
        Frame {
            dt: i.stable_dt.min(0.1),
            camera: CameraInput {
                motion: Vec3::new(
                    axis(egui::Key::D, egui::Key::A),
                    axis(egui::Key::E, egui::Key::Q),
                    axis(egui::Key::W, egui::Key::S),
                ),
                look: if looking {
                    // Mouse right turns right: yaw decreases.
                    Vec2::new(-delta.x * 0.003, -delta.y * 0.003)
                } else {
                    Vec2::ZERO
                },
                zoom: if over_ui {
                    0.0
                } else {
                    i.smooth_scroll_delta.y / 50.0
                },
                fast: i.modifiers.shift,
            },
            pointer: i.pointer.hover_pos().map(|p| Vec2::new(p.x, p.y)),
            pressed: i.pointer.primary_pressed(),
            released: i.pointer.primary_released(),
            undo: ctrl && !i.modifiers.shift && pressed(egui::Key::Z),
            redo: ctrl && (pressed(egui::Key::Y) || (i.modifiers.shift && pressed(egui::Key::Z))),
            delete: pressed(egui::Key::Delete),
            cancel: pressed(egui::Key::Escape),
            toggle_camera: pressed(egui::Key::Tab),
            mode: if ctrl {
                None
            } else if pressed(egui::Key::Num1) {
                Some(HandleMode::Move)
            } else if pressed(egui::Key::Num2) {
                Some(HandleMode::Rotate)
            } else if pressed(egui::Key::Num3) {
                Some(HandleMode::Scale)
            } else {
                None
            },
        }
    })
}

impl Editor {
    /// Handles keyboard and mouse input for one frame: camera movement,
    /// shortcuts (Ctrl+Z, Ctrl+Y, Delete, Escape, Tab, 1-3) and the pointer
    /// in the 3D viewport.
    pub fn handle_input(&mut self, ctx: &egui::Context) {
        let f = gather(ctx);
        self.camera.update(f.dt, &f.camera);
        if f.toggle_camera {
            self.camera.toggle_mode();
        }
        if f.undo {
            self.undo();
        }
        if f.redo {
            self.redo();
        }
        if f.delete {
            self.delete_selection();
        }
        if f.cancel {
            self.cancel();
        }
        if let Some(m) = f.mode {
            self.handle_mode = m;
        }
        let rect = ctx.content_rect();
        let size = Vec2::new(rect.width(), rect.height());
        if let Some(p) = f.pointer {
            let ray = self
                .camera
                .ray_from_pixel(p - Vec2::new(rect.min.x, rect.min.y), size);
            // A press over a panel belongs to the panel.
            if f.pressed && !ctx.egui_wants_pointer_input() {
                self.pointer_press(&ray);
            }
            self.pointer_move(&ray);
        }
        if f.released {
            self.pointer_release();
        }
    }

    /// Sculpt tool settings: operation, radius, strength, and adding or
    /// removing the terrain.
    fn sculpt_settings(&mut self, ui: &mut egui::Ui) {
        if self.map.terrain.is_none() {
            if ui.button("Add terrain").clicked() {
                self.add_terrain();
            }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for op in SculptOp::all() {
                ui.selectable_value(&mut self.sculpt.op, op, op.label());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Radius (m)");
            ui.add(egui::DragValue::new(&mut self.sculpt.radius_m).range(0.5..=64.0));
            ui.label("Strength");
            ui.add(
                egui::DragValue::new(&mut self.sculpt.strength)
                    .range(0.01..=4.0)
                    .speed(0.01),
            );
        });
        if ui.button("Remove terrain").clicked() {
            self.remove_terrain();
        }
    }

    /// The toolbar window: tools, brush kind, camera mode, snapping, undo.
    pub fn toolbar(&mut self, ctx: &egui::Context) {
        egui::Window::new("Editor")
            .anchor(egui::Align2::LEFT_TOP, [8.0, 8.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, Tool::Select, "Select");
                    ui.selectable_value(&mut self.tool, Tool::Create, "Brush");
                    ui.selectable_value(&mut self.tool, Tool::Prop, "Prop");
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tool, Tool::Spawn, "Spawn");
                    ui.selectable_value(&mut self.tool, Tool::Pickup, "Pickup");
                    ui.selectable_value(&mut self.tool, Tool::Zone, "Zone");
                    ui.selectable_value(&mut self.tool, Tool::Light, "Light");
                    ui.selectable_value(&mut self.tool, Tool::Sculpt, "Terrain");
                });
                self.symmetry_combo(ui);
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.panels.show_library, "Library");
                    ui.toggle_value(&mut self.panels.show_gameplay, "Gameplay");
                });
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.workflow.show_validation, "Validation");
                    ui.toggle_value(&mut self.workflow.show_files, "Files");
                    if ui.button("Test").clicked() {
                        self.start_test();
                    }
                });
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.panels.show_meta, "Info");
                    ui.toggle_value(&mut self.panels.show_theme, "Theme");
                    ui.toggle_value(&mut self.panels.show_physics, "Physics");
                });
                if self.tool == Tool::Create {
                    ui.horizontal_wrapped(|ui| {
                        for kind in BrushKind::ALL {
                            ui.selectable_value(
                                &mut self.creation.settings.kind,
                                kind,
                                kind.label(),
                            );
                        }
                    });
                } else if self.tool == Tool::Select {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.handle_mode, HandleMode::Move, "Move");
                        ui.selectable_value(&mut self.handle_mode, HandleMode::Rotate, "Rotate");
                        ui.selectable_value(&mut self.handle_mode, HandleMode::Scale, "Scale");
                    });
                } else if self.tool == Tool::Sculpt {
                    self.sculpt_settings(ui);
                }
                ui.separator();
                let top = self.camera.mode == CameraMode::TopOrtho;
                let mut want_top = top;
                ui.checkbox(&mut want_top, "Top view (Tab)");
                if want_top != top {
                    self.camera.toggle_mode();
                }
                ui.checkbox(&mut self.snap.enabled, "Snap");
                ui.horizontal(|ui| {
                    ui.label("Grid (0.5 m units)");
                    ui.add(egui::DragValue::new(&mut self.snap.grid_units).range(1..=32));
                });
                ui.horizontal(|ui| {
                    ui.label("Angle step (deg)");
                    ui.add(egui::DragValue::new(&mut self.snap.angle_deg).range(1.0..=90.0));
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.history.undo_len() > 0, egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(self.history.redo_len() > 0, egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.redo();
                    }
                    if ui
                        .add_enabled(self.selection.is_some(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.delete_selection();
                    }
                });
                if let Some(name) = self.history.next_undo() {
                    ui.label(format!("Last: {name} ({} steps)", self.history.undo_len()));
                }
            });
    }
}

impl Screen for Editor {
    fn show(&mut self, ctx: &egui::Context, _theme: &Theme) {
        self.toolbar(ctx);
        self.panels(ctx);
        self.workflow_panels(ctx);
        let dt = ctx.input(|i| i.stable_dt.min(0.1));
        self.update_services(dt);
        self.handle_input(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_mapfmt::Brush;

    #[test]
    fn the_screen_runs_headless_and_keeps_the_document() {
        let ctx = egui::Context::default();
        let mut editor = Editor::blank("t");
        editor.add_brush(Brush::Box {
            min: [0, 0, 0],
            max: [4, 4, 4],
            material: "concrete".into(),
        });
        let before = editor.to_ron();
        for _ in 0..3 {
            ctx.run_pass(egui::RawInput::default(), |ctx| {
                editor.show(ctx, &Theme::default());
            })
            .drop_without_applying_deltas();
        }
        assert_eq!(editor.to_ron(), before);
        assert_eq!(editor.history.undo_len(), 1);
    }
}
