//! Gameplay and document tools of the editor: spawns, pickups, zones,
//! lights, prefabs, and the meta, theme and physics blocks. Every placement
//! also writes its mirror twin when a symmetry mode is on, in the same
//! history step.

use faset_core::glam::Vec3;
use faset_mapfmt::{
    Brush, Light, Meta, Physics, Pickup, PickupKind, Placement, Prefab, Prop, Spawn, Team, Theme,
    Zone, ZoneKind,
};

use crate::command::{Command, RemoveBrush, RemoveProp};
use crate::edit::{
    AddItem, AddPrefab, Batch, Item, PlacePrefab, RemoveItem, SetMeta, SetPhysics, SetTheme,
};
use crate::editor::{find_twin, Editor};
use crate::element::{brush_aabb, brush_grid_bounds, translate_brush, Selection};
use crate::math::{ray_aabb, ray_horizontal_plane, Ray};

/// Parameters of the placement tools.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceSettings {
    /// Recipe placed by the prop tool.
    pub recipe: String,
    /// When set, the prop tool places instances of this prefab instead.
    pub prefab: Option<String>,
    /// Facing in degrees (props, spawns; prefabs use quarter turns of it).
    pub yaw: f32,
    pub scale: f32,
    pub team: Team,
    pub pickup: PickupKind,
    pub respawn_s: u32,
    /// Kind of the next zone.
    pub zone: ZoneKind,
    pub light_color: [f32; 3],
    pub light_intensity: f32,
    pub light_range: f32,
    /// Height of the ground plane the placement rays fall back on, metres.
    pub ground_y: f32,
}

impl Default for PlaceSettings {
    fn default() -> Self {
        Self {
            recipe: String::new(),
            prefab: None,
            yaw: 0.0,
            scale: 1.0,
            team: Team::A,
            pickup: PickupKind::Health,
            respawn_s: 20,
            zone: ZoneKind::KillVolume,
            light_color: [1.0, 0.9, 0.7],
            light_intensity: 1.0,
            light_range: 8.0,
            ground_y: 0.0,
        }
    }
}

/// Default prefab suffix of a mirrored definition.
const MIRROR_SUFFIX: &str = "~mirror";

impl Editor {
    /// Where a placement ray lands: the nearest brush, else the ground plane;
    /// snapped to the grid.
    #[must_use]
    pub fn ground_point(&self, ray: &Ray) -> Option<Vec3> {
        let mut best: Option<f32> = None;
        for b in &self.map.brushes {
            if let Some(t) = ray_aabb(ray.origin, ray.dir, &brush_aabb(b)) {
                if best.is_none_or(|bt| t < bt) {
                    best = Some(t);
                }
            }
        }
        // The terrain, when nearer: snapped across, on its surface in height.
        if let (Some(p), Some(t)) = (self.terrain_point(ray), self.map.terrain.as_ref()) {
            if best.is_none_or(|bt| (p - ray.origin).length() < bt) {
                let s = self.snap.snap_position(p);
                return Some(Vec3::new(s.x, t.height_at(s.x, s.z).unwrap_or(p.y), s.z));
            }
        }
        let t = best.or_else(|| ray_horizontal_plane(ray, self.place.ground_y))?;
        Some(self.snap.snap_position(ray.at(t)))
    }

    /// Places whatever the active placement tool places at `point`.
    pub fn place_at(&mut self, point: Vec3) {
        use crate::editor::Tool;
        let at = point.to_array();
        match self.tool {
            Tool::Prop => {
                if let Some(name) = self.place.prefab.clone() {
                    #[allow(clippy::cast_possible_truncation)]
                    let turns = (self.place.yaw / 90.0).round() as i32;
                    let turns = u8::try_from(turns.rem_euclid(4)).unwrap_or(0);
                    self.place_prefab(&name, self.snap.to_grid_pos(point), turns);
                } else if !self.place.recipe.is_empty() {
                    self.add_prop(Prop {
                        recipe: self.place.recipe.clone(),
                        at,
                        yaw: self.place.yaw,
                        scale: self.place.scale,
                    });
                }
            }
            Tool::Spawn => self.add_spawn(Spawn {
                at,
                yaw: self.place.yaw,
                team: self.place.team,
            }),
            Tool::Pickup => self.add_pickup(Pickup {
                kind: self.place.pickup.clone(),
                at,
                respawn_s: self.place.respawn_s,
            }),
            Tool::Light => self.add_light(Light {
                at,
                color: self.place.light_color,
                intensity: self.place.light_intensity,
                range: self.place.light_range,
            }),
            _ => {}
        }
    }

    /// A finished drawn volume: a brush, or a zone with the zone tool.
    pub(crate) fn finish_volume(&mut self, brush: Brush) {
        if self.tool == crate::editor::Tool::Zone {
            if let Brush::Box { min, max, .. } = brush {
                self.add_zone(Zone::Box {
                    min,
                    max,
                    kind: self.place.zone.clone(),
                });
            }
        } else {
            self.add_brush(brush);
        }
    }

    fn add_item<T: Item + 'static>(&mut self, item: T, twin: Option<T>) {
        let twin = twin.map(|t| Box::new(AddItem::new(t)) as Box<dyn Command>);
        self.execute_pair(Box::new(AddItem::new(item)), twin);
    }

    /// Adds a spawn point (and its twin for the other team under symmetry).
    pub fn add_spawn(&mut self, spawn: Spawn) {
        let twin = self.mirror().spawn(&spawn);
        self.add_item(spawn, twin);
    }

    /// Adds a pickup.
    pub fn add_pickup(&mut self, pickup: Pickup) {
        let twin = self.mirror().pickup(&pickup);
        self.add_item(pickup, twin);
    }

    /// Adds a gameplay zone.
    pub fn add_zone(&mut self, zone: Zone) {
        let twin = self.mirror().zone(&zone);
        self.add_item(zone, twin);
    }

    /// Adds a point light.
    pub fn add_light(&mut self, light: Light) {
        let twin = self.mirror().light(&light);
        self.add_item(light, twin);
    }

    fn remove_item<T: Item + 'static>(&mut self, index: usize, twin_of: impl Fn(&T) -> Option<T>) {
        let Some(item) = T::list(&mut self.map).get(index).cloned() else {
            return;
        };
        let twin = twin_of(&item).and_then(|t| find_twin(T::list(&mut self.map), index, &t));
        let mut pairs = vec![(index, item)];
        if let Some(j) = twin {
            pairs.push((j, T::list(&mut self.map)[j].clone()));
        }
        pairs.sort_by_key(|p| std::cmp::Reverse(p.0));
        let cmds = pairs
            .into_iter()
            .map(|(i, it)| Box::new(RemoveItem::new(i, it)) as Box<dyn Command>)
            .collect();
        self.execute(Box::new(Batch::new(&format!("remove {}", T::LABEL), cmds)));
    }

    /// Removes the spawn at `index` and its twin.
    pub fn remove_spawn(&mut self, index: usize) {
        let m = self.mirror();
        self.remove_item(index, |s: &Spawn| m.spawn(s));
    }

    /// Removes the pickup at `index` and its twin.
    pub fn remove_pickup(&mut self, index: usize) {
        let m = self.mirror();
        self.remove_item(index, |s: &Pickup| m.pickup(s));
    }

    /// Removes the zone at `index` and its twin.
    pub fn remove_zone(&mut self, index: usize) {
        let m = self.mirror();
        self.remove_item(index, |s: &Zone| m.zone(s));
    }

    /// Removes the light at `index` and its twin.
    pub fn remove_light(&mut self, index: usize) {
        let m = self.mirror();
        self.remove_item(index, |s: &Light| m.light(s));
    }

    // ---- document blocks ----

    /// Replaces the descriptive block (one history step).
    pub fn set_meta(&mut self, meta: Meta) {
        if meta != self.map.meta {
            let old = self.map.meta.clone();
            self.execute(Box::new(SetMeta::new(old, meta)));
        }
    }

    /// Replaces the theme: sky, weather calendar, ambience and palette. The
    /// change is in the document at once, so every preview that reads
    /// [`Editor::theme_preview`] shows it on the next frame.
    pub fn set_theme(&mut self, theme: Theme) {
        if theme != self.map.theme {
            let old = self.map.theme.clone();
            self.execute(Box::new(SetTheme::new(old, theme)));
        }
    }

    /// Replaces the local physics overrides.
    pub fn set_physics(&mut self, physics: Option<Physics>) {
        if physics != self.map.physics {
            let old = self.map.physics.clone();
            self.execute(Box::new(SetPhysics::new(old, physics)));
        }
    }

    // ---- prefabs ----

    /// Defines a prefab from contents given relative to its own origin.
    pub fn define_prefab(&mut self, name: &str, brushes: Vec<Brush>, props: Vec<Prop>) {
        if self.map.prefabs.iter().any(|p| p.name == name) {
            return;
        }
        self.execute(Box::new(AddPrefab::new(Prefab {
            name: name.to_owned(),
            brushes,
            props,
            place: Vec::new(),
        })));
    }

    /// Instances the prefab `name` at `at` (half-metres) turned by `turns`
    /// quarter turns. Under symmetry the mirror instance is added too, in
    /// the same step: `Rotate180` reuses the definition, the mirrors add a
    /// mirrored definition (`<name>~mirror`) the first time.
    pub fn place_prefab(&mut self, name: &str, at: [i32; 3], turns: u8) {
        let Some(def) = self.map.prefabs.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        let place = Placement { at, turns };
        let mut cmds: Vec<Box<dyn Command>> = vec![Box::new(PlacePrefab::new(&def, place))];
        if let Some((twin, same_definition)) = self.mirror().placement(&place) {
            if same_definition {
                if twin != place {
                    cmds.push(Box::new(PlacePrefab::new(&def, twin)));
                }
            } else {
                let existing = self
                    .map
                    .prefabs
                    .iter()
                    .find(|p| p.name == format!("{name}{MIRROR_SUFFIX}"))
                    .cloned();
                let mirrored = if let Some(m) = existing {
                    m
                } else if let Some(m) = self.mirror().prefab_definition(&def, MIRROR_SUFFIX) {
                    cmds.insert(0, Box::new(AddPrefab::new(m.clone())));
                    m
                } else {
                    return;
                };
                cmds.push(Box::new(PlacePrefab::new(&mirrored, twin)));
            }
        }
        let batch = Batch::new("place prefab", cmds);
        self.execute(Box::new(batch));
    }

    /// Turns the selected brush or prop into a prefab named `name`, placed
    /// where it stood (one step). Symmetry is not applied here.
    pub fn prefab_from_selection(&mut self, name: &str) {
        if self.map.prefabs.iter().any(|p| p.name == name) {
            return;
        }
        let Some(sel) = self.selection else { return };
        let (origin, brushes, props, remove): ([i32; 3], Vec<Brush>, Vec<Prop>, Box<dyn Command>) =
            match sel {
                Selection::Brush(i) => {
                    let Some(b) = self.map.brushes.get(i).cloned() else {
                        return;
                    };
                    let origin = brush_grid_bounds(&b).0;
                    let rel = translate_brush(&b, origin.map(|v| -v));
                    (
                        origin,
                        vec![rel],
                        Vec::new(),
                        Box::new(RemoveBrush::new(i, b)),
                    )
                }
                Selection::Prop(i) => {
                    let Some(p) = self.map.props.get(i).cloned() else {
                        return;
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let origin = p.at.map(|v| (v * 2.0).round() as i32);
                    let mut rel = p.clone();
                    #[allow(clippy::cast_precision_loss)]
                    for k in 0..3 {
                        rel.at[k] -= origin[k] as f32 * 0.5;
                    }
                    (
                        origin,
                        Vec::new(),
                        vec![rel],
                        Box::new(RemoveProp::new(i, p)),
                    )
                }
            };
        let prefab = Prefab {
            name: name.to_owned(),
            brushes,
            props,
            place: vec![Placement {
                at: origin,
                turns: 0,
            }],
        };
        self.execute(Box::new(Batch::new(
            "make prefab",
            vec![remove, Box::new(AddPrefab::new(prefab))],
        )));
        self.selection = None;
    }
}

/// What the theme looks like, for an immediate preview next to the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemePreview {
    /// Sky preset in effect (the ambience default when the map sets none).
    pub sky: String,
    /// Weather at the preview time.
    pub weather: String,
    /// The ambience profile, when it exists in the renderer.
    pub profile: Option<faset_render::AmbienceProfile>,
}

impl Editor {
    /// The theme as the renderer would resolve it at `time_s` seconds into
    /// the match.
    #[must_use]
    pub fn theme_preview(&self, time_s: f32) -> ThemePreview {
        let theme = &self.map.theme;
        let weather = theme
            .weather
            .iter()
            .filter(|w| w.from_s <= time_s)
            .max_by(|a, b| a.from_s.total_cmp(&b.from_s))
            .map_or_else(|| "clear".to_owned(), |w| w.weather.clone());
        ThemePreview {
            sky: if theme.sky.is_empty() {
                theme.ambience.clone()
            } else {
                theme.sky.clone()
            },
            weather,
            profile: faset_render::AmbienceProfile::builtin(&theme.ambience),
        }
    }
}
