//! The editor's validation, test and file workflow: per-frame services and
//! the panels that expose them.

use faset_core::glam::Vec3;
use faset_ui::egui;

use crate::editor::Editor;
use crate::files::FileService;
use crate::test_session::TestSession;
use crate::validation::Severity;

/// UI-only state of the validation and file panels.
#[derive(Debug, Clone)]
pub struct WorkflowUi {
    pub show_validation: bool,
    pub show_files: bool,
    /// Destination typed in the export box.
    pub export_path: String,
    /// Source typed in the import box.
    pub import_path: String,
    /// Result of the last file or test action, shown in the panel.
    pub status: String,
}

impl Default for WorkflowUi {
    fn default() -> Self {
        Self {
            show_validation: true,
            show_files: false,
            export_path: String::new(),
            import_path: String::new(),
            status: String::new(),
        }
    }
}

impl Editor {
    /// Plugs in the file service (save, export, import, autosave).
    pub fn set_files(&mut self, files: FileService) {
        self.files = Some(files);
    }

    /// Per-frame services: re-validates after edits and autosaves every
    /// 60 s. `dt` is the frame time in seconds (a simulated clock in tests).
    pub fn update_services(&mut self, dt: f32) {
        self.validation.update(&self.map, dt);
        if let Some(files) = &mut self.files {
            if let Err(e) = files.tick(dt, &self.map) {
                tracing::warn!(error = %e, "autosave failed");
                self.workflow.status = format!("Autosave failed: {e}");
            }
        }
    }

    /// Clicks issue `index` of the validation panel: the camera moves onto
    /// it. Returns the point framed.
    pub fn click_issue(&mut self, index: usize) -> Option<Vec3> {
        self.validation.click(index, &mut self.camera)
    }

    /// The Save button: writes the map to the `maps/` folder.
    pub fn save(&mut self) {
        let result = match &self.files {
            Some(files) => files.save(&self.map).map(|p| p.display().to_string()),
            None => {
                self.workflow.status = "No data folder configured".into();
                return;
            }
        };
        self.workflow.status = match result {
            Ok(path) => format!("Saved to {path}"),
            Err(e) => format!("Save failed: {e}"),
        };
    }

    /// The Export button: copies the map to the path in the export box.
    pub fn export_to_box(&mut self) {
        let dest = std::path::PathBuf::from(self.workflow.export_path.trim());
        let Some(files) = &self.files else { return };
        self.workflow.status = match files.export(&self.map, &dest) {
            Ok(()) => format!("Exported to {}", dest.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    /// The Import button: opens the map at the path in the import box and
    /// copies it into the library.
    pub fn import_from_box(&mut self) {
        let src = std::path::PathBuf::from(self.workflow.import_path.trim());
        let Some(files) = &self.files else { return };
        match files.import(&src) {
            Ok(imported) => {
                self.workflow.status = format!("Imported to {}", imported.path.display());
                self.load(imported.map);
            }
            Err(e) => {
                let rule = e.rule().map(|r| format!("rule {r}: ")).unwrap_or_default();
                self.workflow.status = format!("Import failed, {rule}{e}");
            }
        }
    }

    /// The Test button: starts an integrated server on the document with the
    /// player at the camera position. The application takes the session with
    /// [`Editor::take_test`] and connects its player to it.
    pub fn start_test(&mut self) {
        let at = match self.camera.mode {
            crate::camera::CameraMode::FreeFly => self.camera.eye,
            // Top view: drop the player on the ground under the view centre,
            // a little above the floor.
            crate::camera::CameraMode::TopOrtho => {
                let t = self.camera.target();
                Vec3::new(t.x, 2.0, t.z)
            }
        };
        match TestSession::start(&self.map, at) {
            Ok(session) => {
                self.workflow.status = "Test started".into();
                self.test = Some(session);
            }
            Err(e) => self.workflow.status = format!("Test failed: {e}"),
        }
    }

    /// Hands the running test over to the application.
    pub fn take_test(&mut self) -> Option<TestSession> {
        self.test.take()
    }

    /// The validation and file windows the toolbar switched on.
    pub fn workflow_panels(&mut self, ctx: &egui::Context) {
        if self.workflow.show_validation {
            self.validation_window(ctx);
        }
        if self.workflow.show_files {
            self.files_window(ctx);
        }
    }

    fn validation_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut clicked = None;
        egui::Window::new("Validation")
            .open(&mut open)
            .default_width(340.0)
            .anchor(egui::Align2::LEFT_BOTTOM, [8.0, -8.0])
            .show(ctx, |ui| {
                let (text, color) = if self.validation.is_ok() {
                    ("All rules pass", egui::Color32::LIGHT_GREEN)
                } else {
                    ("Rules broken", egui::Color32::LIGHT_RED)
                };
                ui.colored_label(color, text);
                let stats = self.validation.stats();
                ui.small(format!(
                    "{} triangles, {:.0} m2 playable, longest sight {:.0} m, cover {:.0} %",
                    stats.triangles,
                    stats.playable_surface_m2,
                    stats.longest_line_of_sight_m,
                    stats.cover_density * 100.0
                ));
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (i, issue) in self.validation.issues().iter().enumerate() {
                            let tag = match issue.severity {
                                Severity::Error => "error",
                                Severity::Warning => "warning",
                            };
                            let label = format!(
                                "[rule {}] {} {}: {}",
                                issue.rule, tag, issue.element, issue.message
                            );
                            let selected = self.validation.selected == Some(i);
                            if ui.selectable_label(selected, label).clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
            });
        if let Some(i) = clicked {
            self.click_issue(i);
        }
        self.workflow.show_validation = open;
    }

    fn files_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Files")
            .open(&mut open)
            .default_width(320.0)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-8.0, -8.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.save();
                    }
                    if ui.button("Test").clicked() {
                        self.start_test();
                    }
                });
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.workflow.export_path);
                    if ui.button("Export").clicked() {
                        self.export_to_box();
                    }
                });
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.workflow.import_path);
                    if ui.button("Import").clicked() {
                        self.import_from_box();
                    }
                });
                if !self.workflow.status.is_empty() {
                    ui.label(&self.workflow.status);
                }
            });
        self.workflow.show_files = open;
    }
}
