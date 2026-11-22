//! egui panels: library, gameplay settings, map info, theme and physics.
//!
//! Text and number fields edit a draft; the draft becomes one command when
//! the field loses focus or the drag ends, so typing does not flood the
//! history. Choices made in combo boxes apply at once.

use std::collections::HashMap;

use faset_mapfmt::{Meta, Physics, PickupKind, Symmetry, Team, Theme, WeatherSlot, ZoneKind};
use faset_ui::egui;

use crate::editor::{Editor, Tool};
use crate::library::{AMBIENCES, MATERIALS, PALETTES, SKIES, WEATHERS};

/// A value being edited in a panel, kept until the edit ends.
#[derive(Debug)]
pub struct Draft<T>(Option<T>);

impl<T> Default for Draft<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Clone + PartialEq> Draft<T> {
    fn get(&self, doc: &T) -> T {
        self.0.clone().unwrap_or_else(|| doc.clone())
    }

    /// Keeps `value` while the user is still editing; returns it once the
    /// edit ended and it differs from the document.
    fn finish(&mut self, value: T, busy: bool, doc: &T) -> Option<T> {
        if busy {
            self.0 = Some(value);
            None
        } else {
            self.0 = None;
            (value != *doc).then_some(value)
        }
    }
}

/// Which list the library panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryTab {
    #[default]
    Props,
    Materials,
    Palettes,
    Prefabs,
}

/// UI-only state of the panels.
#[derive(Default)]
pub struct PanelState {
    pub show_library: bool,
    pub show_gameplay: bool,
    pub show_meta: bool,
    pub show_theme: bool,
    pub show_physics: bool,
    pub tab: LibraryTab,
    pub search: String,
    pub prefab_name: String,
    textures: HashMap<String, egui::TextureHandle>,
    meta: Draft<Meta>,
    theme: Draft<Theme>,
    physics: Draft<Option<Physics>>,
}

impl PanelState {
    /// Opens every panel (used by tests and the demo).
    pub fn show_all(&mut self) {
        self.show_library = true;
        self.show_gameplay = true;
        self.show_meta = true;
        self.show_theme = true;
        self.show_physics = true;
    }
}

fn combo(ui: &mut egui::Ui, id: &str, current: &mut String, options: &[&str]) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if current.is_empty() {
            "(default)"
        } else {
            current.as_str()
        })
        .show_ui(ui, |ui| {
            for o in options {
                ui.selectable_value(current, (*o).to_owned(), *o);
            }
        });
}

fn busy(r: &egui::Response) -> bool {
    r.has_focus() || r.dragged()
}

impl Editor {
    /// Shows the panels the toolbar switched on.
    pub fn panels(&mut self, ctx: &egui::Context) {
        if self.panels.show_library {
            self.library_panel(ctx);
        }
        if self.panels.show_gameplay {
            self.gameplay_panel(ctx);
        }
        if self.panels.show_meta {
            self.meta_panel(ctx);
        }
        if self.panels.show_theme {
            self.theme_panel(ctx);
        }
        if self.panels.show_physics {
            self.physics_panel(ctx);
        }
    }

    fn library_panel(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Library")
            .open(&mut open)
            .default_width(300.0)
            .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.panels.tab, LibraryTab::Props, "Props");
                    ui.selectable_value(&mut self.panels.tab, LibraryTab::Materials, "Materials");
                    ui.selectable_value(&mut self.panels.tab, LibraryTab::Palettes, "Palettes");
                    ui.selectable_value(&mut self.panels.tab, LibraryTab::Prefabs, "Prefabs");
                });
                ui.separator();
                match self.panels.tab {
                    LibraryTab::Props => self.props_tab(ui),
                    LibraryTab::Materials => {
                        for m in MATERIALS {
                            let on = self.creation.settings.material == m;
                            if ui.selectable_label(on, m).clicked() {
                                self.creation.settings.material = m.to_owned();
                            }
                        }
                    }
                    LibraryTab::Palettes => {
                        for p in PALETTES {
                            let on = self.map.theme.palette == p;
                            if ui.selectable_label(on, p).clicked() {
                                let mut t = self.map.theme.clone();
                                p.clone_into(&mut t.palette);
                                self.set_theme(t);
                            }
                        }
                    }
                    LibraryTab::Prefabs => self.prefabs_tab(ui),
                }
            });
        self.panels.show_library = open;
    }

    fn props_tab(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.panels.search).hint_text("search id or tag"));
        let ctx = ui.ctx().clone();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for entry in self.library.search(&self.panels.search) {
                        let tex =
                            self.panels
                                .textures
                                .entry(entry.id.clone())
                                .or_insert_with(|| {
                                    let t = &entry.thumbnail;
                                    ctx.load_texture(
                                        format!("thumb:{}", entry.id),
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [t.size, t.size],
                                            &t.rgba,
                                        ),
                                        egui::TextureOptions::LINEAR,
                                    )
                                });
                        let image = egui::Image::new((tex.id(), egui::vec2(56.0, 56.0)))
                            .sense(egui::Sense::click());
                        let r = ui.add(image).on_hover_text(&entry.id);
                        if r.clicked() {
                            picked = Some(entry.id.clone());
                        }
                    }
                });
            });
        if let Some(id) = picked {
            self.place.recipe = id;
            self.place.prefab = None;
            self.tool = Tool::Prop;
        }
        ui.label(format!(
            "{} props, selected: {}",
            self.library.len(),
            if self.place.recipe.is_empty() {
                "none"
            } else {
                &self.place.recipe
            }
        ));
    }

    fn prefabs_tab(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        for p in &self.map.prefabs {
            ui.horizontal(|ui| {
                let on = self.place.prefab.as_deref() == Some(p.name.as_str());
                if ui.selectable_label(on, &p.name).clicked() {
                    pick = Some(p.name.clone());
                }
                ui.label(format!("{} instances", p.place.len()));
            });
        }
        if let Some(name) = pick {
            self.place.prefab = Some(name);
            self.tool = Tool::Prop;
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.panels.prefab_name)
                    .hint_text("prefab name")
                    .desired_width(120.0),
            );
            if ui
                .add_enabled(
                    self.selection.is_some() && !self.panels.prefab_name.is_empty(),
                    egui::Button::new("From selection"),
                )
                .clicked()
            {
                let name = self.panels.prefab_name.clone();
                self.prefab_from_selection(&name);
            }
        });
    }

    fn gameplay_panel(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Gameplay")
            .open(&mut open)
            .anchor(egui::Align2::LEFT_BOTTOM, [8.0, -8.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Facing (deg)");
                    ui.add(egui::DragValue::new(&mut self.place.yaw).speed(1.0));
                });
                match self.tool {
                    Tool::Spawn => {
                        ui.horizontal(|ui| {
                            for t in [Team::A, Team::B, Team::Any] {
                                ui.selectable_value(&mut self.place.team, t, format!("{t:?}"));
                            }
                        });
                    }
                    Tool::Pickup => {
                        ui.horizontal_wrapped(|ui| {
                            for k in [PickupKind::Health, PickupKind::Armor, PickupKind::Ammo] {
                                let label = format!("{k:?}");
                                ui.selectable_value(&mut self.place.pickup, k, label);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Respawn (s)");
                            ui.add(egui::DragValue::new(&mut self.place.respawn_s).range(1..=600));
                        });
                    }
                    Tool::Zone => self.zone_settings(ui),
                    Tool::Light => {
                        ui.horizontal(|ui| {
                            ui.color_edit_button_rgb(&mut self.place.light_color);
                            ui.label("range");
                            ui.add(
                                egui::DragValue::new(&mut self.place.light_range).range(0.5..=64.0),
                            );
                            ui.label("intensity");
                            ui.add(
                                egui::DragValue::new(&mut self.place.light_intensity)
                                    .range(0.0..=16.0)
                                    .speed(0.05),
                            );
                        });
                    }
                    Tool::Prop => {
                        ui.horizontal(|ui| {
                            ui.label("Scale");
                            ui.add(
                                egui::DragValue::new(&mut self.place.scale)
                                    .range(0.1..=8.0)
                                    .speed(0.05),
                            );
                        });
                    }
                    _ => {
                        ui.label("Pick a placement tool in the toolbar.");
                    }
                }
                ui.separator();
                self.element_lists(ui);
            });
        self.panels.show_gameplay = open;
    }

    fn zone_settings(&mut self, ui: &mut egui::Ui) {
        let kinds: [(&str, ZoneKind); 6] = [
            ("KillVolume", ZoneKind::KillVolume),
            ("Water", ZoneKind::Water),
            (
                "JumpPad",
                ZoneKind::JumpPad {
                    velocity: [0.0, 12.0, 0.0],
                },
            ),
            (
                "Teleporter",
                ZoneKind::Teleporter {
                    target: [0.0, 0.0, 0.0],
                    yaw: 0.0,
                },
            ),
            ("LowGravity", ZoneKind::LowGravity { scale: 0.4 }),
            ("Objective", ZoneKind::Objective { id: "a".to_owned() }),
        ];
        ui.horizontal_wrapped(|ui| {
            for (label, kind) in &kinds {
                let on = std::mem::discriminant(&self.place.zone) == std::mem::discriminant(kind);
                if ui.selectable_label(on, *label).clicked() && !on {
                    self.place.zone = kind.clone();
                }
            }
        });
        match &mut self.place.zone {
            ZoneKind::JumpPad { velocity } => {
                ui.horizontal(|ui| {
                    for v in velocity.iter_mut() {
                        ui.add(egui::DragValue::new(v).speed(0.1));
                    }
                    ui.label("m/s");
                });
            }
            ZoneKind::Teleporter { target, yaw } => {
                ui.horizontal(|ui| {
                    for v in target.iter_mut() {
                        ui.add(egui::DragValue::new(v).speed(0.1));
                    }
                    ui.add(egui::DragValue::new(yaw).suffix(" deg"));
                });
            }
            ZoneKind::LowGravity { scale } => {
                ui.add(egui::DragValue::new(scale).range(0.0..=1.0).speed(0.01));
            }
            ZoneKind::Objective { id } => {
                ui.text_edit_singleline(id);
            }
            ZoneKind::KillVolume | ZoneKind::Water => {}
        }
        ui.label("Draw the zone as a box on the ground, like a volume.");
    }

    /// Lists of the placed gameplay elements, each with a delete button.
    fn element_lists(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<(u8, usize)> = None;
        egui::CollapsingHeader::new(format!("Spawns ({})", self.map.spawns.len())).show(ui, |ui| {
            for (i, s) in self.map.spawns.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{:?} at {:.1}, {:.1}, {:.1}",
                        s.team, s.at[0], s.at[1], s.at[2]
                    ));
                    if ui.small_button("x").clicked() {
                        remove = Some((0, i));
                    }
                });
            }
        });
        egui::CollapsingHeader::new(format!("Pickups ({})", self.map.pickups.len())).show(
            ui,
            |ui| {
                for (i, p) in self.map.pickups.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{:?} every {} s", p.kind, p.respawn_s));
                        if ui.small_button("x").clicked() {
                            remove = Some((1, i));
                        }
                    });
                }
            },
        );
        egui::CollapsingHeader::new(format!("Zones ({})", self.map.zones.len())).show(ui, |ui| {
            for (i, z) in self.map.zones.iter().enumerate() {
                ui.horizontal(|ui| {
                    let faset_mapfmt::Zone::Box { kind, .. } = z;
                    ui.label(format!("{kind:?}"));
                    if ui.small_button("x").clicked() {
                        remove = Some((2, i));
                    }
                });
            }
        });
        egui::CollapsingHeader::new(format!("Lights ({})", self.map.lights.len())).show(ui, |ui| {
            for (i, l) in self.map.lights.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "range {:.1} m at {:.1}, {:.1}, {:.1}",
                        l.range, l.at[0], l.at[1], l.at[2]
                    ));
                    if ui.small_button("x").clicked() {
                        remove = Some((3, i));
                    }
                });
            }
        });
        match remove {
            Some((0, i)) => self.remove_spawn(i),
            Some((1, i)) => self.remove_pickup(i),
            Some((2, i)) => self.remove_zone(i),
            Some((3, i)) => self.remove_light(i),
            _ => {}
        }
    }

    fn meta_panel(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut commit = None;
        egui::Window::new("Map info")
            .open(&mut open)
            .default_pos([320.0, 40.0])
            .show(ctx, |ui| {
                let mut m = self.panels.meta.get(&self.map.meta);
                let mut is_busy = false;
                egui::Grid::new("meta").num_columns(2).show(ui, |ui| {
                    ui.label("Name");
                    is_busy |= busy(&ui.text_edit_singleline(&mut m.name));
                    ui.end_row();
                    ui.label("Author");
                    is_busy |= busy(&ui.text_edit_singleline(&mut m.author));
                    ui.end_row();
                    ui.label("Description");
                    is_busy |= busy(&ui.text_edit_multiline(&mut m.description));
                    ui.end_row();
                    ui.label("Modes");
                    ui.horizontal(|ui| {
                        for mode in ["fps", "race"] {
                            let mut on = m.modes.iter().any(|x| x == mode);
                            if ui.checkbox(&mut on, mode).changed() {
                                m.modes.retain(|x| x != mode);
                                if on {
                                    m.modes.push(mode.to_owned());
                                }
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Players");
                    ui.horizontal(|ui| {
                        is_busy |=
                            busy(&ui.add(egui::DragValue::new(&mut m.players.0).range(1..=64)));
                        ui.label("to");
                        is_busy |=
                            busy(&ui.add(egui::DragValue::new(&mut m.players.1).range(1..=64)));
                    });
                    ui.end_row();
                });
                commit = self.panels.meta.finish(m, is_busy, &self.map.meta);
            });
        if let Some(m) = commit {
            self.set_meta(m);
        }
        self.panels.show_meta = open;
    }

    fn theme_panel(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut commit = None;
        egui::Window::new("Theme")
            .open(&mut open)
            .default_pos([320.0, 240.0])
            .show(ctx, |ui| {
                let mut t = self.panels.theme.get(&self.map.theme);
                let mut is_busy = false;
                egui::Grid::new("theme").num_columns(2).show(ui, |ui| {
                    ui.label("Ambience");
                    combo(ui, "ambience", &mut t.ambience, &AMBIENCES);
                    ui.end_row();
                    ui.label("Sky");
                    combo(ui, "sky", &mut t.sky, &SKIES);
                    ui.end_row();
                    ui.label("Palette");
                    combo(ui, "palette", &mut t.palette, &PALETTES);
                    ui.end_row();
                });
                ui.label("Weather calendar");
                let mut drop = None;
                for (i, slot) in t.weather.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        is_busy |= busy(
                            &ui.add(
                                egui::DragValue::new(&mut slot.from_s)
                                    .range(0.0..=36_000.0)
                                    .suffix(" s"),
                            ),
                        );
                        combo(ui, &format!("weather{i}"), &mut slot.weather, &WEATHERS);
                        if ui.small_button("x").clicked() {
                            drop = Some(i);
                        }
                    });
                }
                if let Some(i) = drop {
                    t.weather.remove(i);
                }
                if ui.button("Add weather").clicked() {
                    let from_s = t.weather.last().map_or(0.0, |w| w.from_s + 120.0);
                    t.weather.push(WeatherSlot {
                        from_s,
                        weather: "clear".to_owned(),
                    });
                }
                ui.separator();
                let preview = {
                    let saved = std::mem::replace(&mut self.map.theme, t.clone());
                    let p = self.theme_preview(0.0);
                    self.map.theme = saved;
                    p
                };
                ui.label(format!(
                    "Preview: sky {}, weather {}",
                    preview.sky, preview.weather
                ));
                if let Some(p) = &preview.profile {
                    let c = p.sky.horizon;
                    let z = p.sky.zenith;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(160.0, 24.0), egui::Sense::hover());
                    let to = |c: [f32; 3]| {
                        egui::Color32::from_rgb(
                            (c[0].clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                            (c[1].clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                            (c[2].clamp(0.0, 1.0).sqrt() * 255.0) as u8,
                        )
                    };
                    let half = rect.height() * 0.5;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), half)),
                        0.0,
                        to(z),
                    );
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            rect.min + egui::vec2(0.0, half),
                            egui::vec2(rect.width(), half),
                        ),
                        0.0,
                        to(c),
                    );
                }
                commit = self.panels.theme.finish(t, is_busy, &self.map.theme);
            });
        if let Some(t) = commit {
            self.set_theme(t);
        }
        self.panels.show_theme = open;
    }

    fn physics_panel(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut commit = None;
        egui::Window::new("Physics")
            .open(&mut open)
            .default_pos([320.0, 480.0])
            .show(ctx, |ui| {
                let mut p = self.panels.physics.get(&self.map.physics);
                let mut is_busy = false;
                let mut on = p.is_some();
                if ui.checkbox(&mut on, "Local overrides").changed() {
                    p = on.then(Physics::default);
                }
                if let Some(ph) = &mut p {
                    let mut gravity = ph.gravity;
                    let mut set_gravity = gravity.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut set_gravity, "Gravity").changed() {
                            gravity = set_gravity.then_some(9.81);
                        }
                        if let Some(g) = &mut gravity {
                            is_busy |= busy(
                                &ui.add(
                                    egui::DragValue::new(g)
                                        .range(0.0..=40.0)
                                        .speed(0.05)
                                        .suffix(" m/s2"),
                                ),
                            );
                        }
                    });
                    ph.gravity = gravity;
                    let mut speed = ph.speed_scale;
                    let mut set_speed = speed.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut set_speed, "Speed scale").changed() {
                            speed = set_speed.then_some(1.0);
                        }
                        if let Some(s) = &mut speed {
                            is_busy |=
                                busy(&ui.add(egui::DragValue::new(s).range(0.1..=4.0).speed(0.01)));
                        }
                    });
                    ph.speed_scale = speed;
                }
                commit = self.panels.physics.finish(p, is_busy, &self.map.physics);
            });
        if let Some(p) = commit {
            self.set_physics(p);
        }
        self.panels.show_physics = open;
    }

    /// The symmetry selector shown in the toolbar.
    pub(crate) fn symmetry_combo(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Symmetry");
            egui::ComboBox::from_id_salt("symmetry")
                .selected_text(format!("{:?}", self.symmetry))
                .show_ui(ui, |ui| {
                    for s in [
                        Symmetry::None,
                        Symmetry::MirrorX,
                        Symmetry::MirrorZ,
                        Symmetry::Rotate180,
                    ] {
                        ui.selectable_value(&mut self.symmetry, s, format!("{s:?}"));
                    }
                });
        });
    }
}
