//! The editor state: document, history, camera, selection and tools.

use faset_core::glam::Vec3;
use faset_mapfmt::{Brush, MapFile, Meta, Prop, Symmetry, Theme};

use crate::edit::Batch;
use crate::files::FileService;
use crate::gameplay::PlaceSettings;
use crate::library::{Library, LibraryBounds};
use crate::panels::PanelState;
use crate::symmetry::Mirror;
use crate::test_session::TestSession;
use crate::validation::ValidationPanel;
use crate::workflow::WorkflowUi;

use crate::camera::{CameraMode, EditorCamera};
use crate::command::{
    AddBrush, AddProp, Command, History, RemoveBrush, RemoveProp, ReplaceBrush, ReplaceProp,
};
use crate::create::Creation;
use crate::element::{
    brush_aabb, element_centre, ray_prop, Element, PropBounds, Selection, UnitPropBounds,
};
use crate::handles::{handle_length, pick_handle, Drag, HandleMode};
use crate::math::{ray_aabb, Aabb, Ray};
use crate::mesh::{Block, MeshCache};
use crate::sculpt::{dab, ray_terrain, terrain_for, SculptOp, SculptSettings, Stroke, TerrainEdit};
use crate::snap::SnapSettings;

/// The active tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Pick elements and drag their handles.
    #[default]
    Select,
    /// Draw new volumes (the brush tool).
    Create,
    /// Place props, or prefab instances when a prefab is chosen.
    Prop,
    /// Place player spawn points.
    Spawn,
    /// Place pickups.
    Pickup,
    /// Draw gameplay zones (a box on the ground, like a brush).
    Zone,
    /// Place point lights.
    Light,
    /// Sculpt and paint the terrain (lot 6.10).
    Sculpt,
}

/// A valid, empty map to start editing from.
#[must_use]
pub fn new_map(name: &str) -> MapFile {
    MapFile {
        format_version: faset_mapfmt::FORMAT_VERSION,
        meta: Meta {
            name: name.to_owned(),
            author: String::new(),
            description: String::new(),
            modes: vec!["fps".to_owned()],
            players: (2, 8),
        },
        theme: Theme {
            ambience: "sunset".to_owned(),
            sky: String::new(),
            weather: Vec::new(),
            palette: "industrial".to_owned(),
        },
        physics: None,
        symmetry: Symmetry::None,
        prefabs: Vec::new(),
        brushes: Vec::new(),
        props: Vec::new(),
        spawns: Vec::new(),
        pickups: Vec::new(),
        zones: Vec::new(),
        lights: Vec::new(),
        terrain: None,
        vegetation: None,
    }
}

/// The editor: a client state whose document is directly the
/// [`MapFile`] that gets saved, so what is edited is what is saved.
pub struct Editor {
    /// The document.
    pub map: MapFile,
    pub history: History,
    pub camera: EditorCamera,
    pub snap: SnapSettings,
    pub tool: Tool,
    pub handle_mode: HandleMode,
    pub creation: Creation,
    pub selection: Option<Selection>,
    /// Mesh chunks of the document; call [`Editor::rebuild_meshes`] each
    /// frame before drawing.
    pub meshes: MeshCache,
    /// Prop bounds used for picking; the library's real bounds once one is
    /// set with [`Editor::set_library`].
    pub bounds: Box<dyn PropBounds>,
    /// The asset library (props with thumbnails).
    pub library: Library,
    /// Symmetry mode of the session. Decision: it is an editor setting, not
    /// the file's `symmetry` field: every command also writes its mirror twin
    /// into the document, so the saved file already holds both halves and
    /// declares `Symmetry::None` (declaring it as well would mirror twice on
    /// load).
    pub symmetry: Symmetry,
    /// Parameters of the placement tools.
    pub place: PlaceSettings,
    /// State of the egui panels (search text, drafts, textures).
    pub panels: PanelState,
    /// Live validation results.
    pub validation: ValidationPanel,
    /// Save, export, import and autosave, once a data folder is set.
    pub files: Option<FileService>,
    /// The running immediate test, until the application takes it.
    pub test: Option<TestSession>,
    /// State of the validation and file panels.
    pub workflow: WorkflowUi,
    /// Parameters of the sculpt tool.
    pub sculpt: SculptSettings,
    drag: Option<Drag>,
    stroke: Option<Stroke>,
}

/// The editor under the name the lot's interface lists.
pub type EditorState = Editor;

impl Editor {
    /// An editor over an existing map.
    #[must_use]
    pub fn new(map: MapFile) -> Self {
        Self {
            map,
            history: History::new(),
            camera: EditorCamera::default(),
            snap: SnapSettings::default(),
            tool: Tool::default(),
            handle_mode: HandleMode::default(),
            creation: Creation::default(),
            selection: None,
            meshes: MeshCache::new(),
            bounds: Box::new(UnitPropBounds),
            library: Library::new(),
            symmetry: Symmetry::None,
            place: PlaceSettings::default(),
            panels: PanelState::default(),
            validation: ValidationPanel::new(),
            files: None,
            test: None,
            workflow: WorkflowUi::default(),
            sculpt: SculptSettings::default(),
            drag: None,
            stroke: None,
        }
    }

    /// An editor over a new empty map.
    #[must_use]
    pub fn blank(name: &str) -> Self {
        Self::new(new_map(name))
    }

    /// Plugs in the asset library: props are then picked and placed with
    /// their real bounds.
    pub fn set_library(&mut self, library: Library) {
        self.bounds = Box::new(library.prop_bounds());
        self.library = library;
    }

    /// Real bounds provider of the current library.
    #[must_use]
    pub fn library_bounds(&self) -> LibraryBounds {
        self.library.prop_bounds()
    }

    /// The twin maker of the current symmetry mode.
    #[must_use]
    pub fn mirror(&self) -> Mirror {
        Mirror(self.symmetry)
    }

    /// Executes `primary` and, when there is one, its mirror `twin`, as one
    /// history step.
    pub(crate) fn execute_pair(
        &mut self,
        primary: Box<dyn Command>,
        twin: Option<Box<dyn Command>>,
    ) {
        match twin {
            Some(twin) => {
                let name = format!("{} (mirrored)", primary.name());
                self.execute(Box::new(Batch::new(&name, vec![primary, twin])));
            }
            None => self.execute(primary),
        }
    }

    /// The document in its canonical file form, exactly what gets saved.
    #[must_use]
    pub fn to_ron(&self) -> String {
        faset_mapfmt::to_ron(&self.map)
    }

    /// Replaces the document (after a load): history, selection and meshes
    /// start over.
    pub fn load(&mut self, map: MapFile) {
        self.cancel();
        self.map = map;
        self.history.clear();
        self.selection = None;
        self.meshes.mark_all_dirty();
    }

    // ---- commands ----

    /// Applies a command and records it in the history.
    pub fn execute(&mut self, command: Box<dyn Command>) {
        self.cancel_drag();
        let touched = self.history.push(command, &mut self.map);
        self.mark(&touched);
        self.fix_selection();
    }

    /// Reverts the last command. Returns false when there was none.
    pub fn undo(&mut self) -> bool {
        self.cancel_drag();
        self.cancel_stroke();
        let Some(touched) = self.history.undo(&mut self.map) else {
            return false;
        };
        self.mark(&touched);
        self.fix_selection();
        true
    }

    /// Re-applies the last undone command. Returns false when there was none.
    pub fn redo(&mut self) -> bool {
        self.cancel_drag();
        self.cancel_stroke();
        let Some(touched) = self.history.redo(&mut self.map) else {
            return false;
        };
        self.mark(&touched);
        self.fix_selection();
        true
    }

    /// Adds a brush through a command and selects it.
    pub fn add_brush(&mut self, brush: Brush) {
        let twin = self.mirror().brush(&brush);
        let count = 1 + usize::from(twin.is_some());
        let twin = twin.map(|t| Box::new(AddBrush::new(t)) as Box<dyn Command>);
        self.execute_pair(Box::new(AddBrush::new(brush)), twin);
        self.selection = Some(Selection::Brush(self.map.brushes.len() - count));
    }

    /// Adds a prop through a command and selects it.
    pub fn add_prop(&mut self, prop: Prop) {
        let twin = self.mirror().prop(&prop);
        let count = 1 + usize::from(twin.is_some());
        let twin = twin.map(|t| Box::new(AddProp::new(t)) as Box<dyn Command>);
        self.execute_pair(Box::new(AddProp::new(prop)), twin);
        self.selection = Some(Selection::Prop(self.map.props.len() - count));
    }

    /// Deletes the selected element through a command.
    pub fn delete_selection(&mut self) {
        let Some(sel) = self.selection else { return };
        let mirror = self.mirror();
        match sel {
            Selection::Brush(i) => {
                let Some(b) = self.map.brushes.get(i).cloned() else {
                    return;
                };
                let twin = mirror
                    .brush(&b)
                    .and_then(|t| find_twin(&self.map.brushes, i, &t));
                let mut pairs = vec![(i, b)];
                pairs.extend(twin.map(|j| (j, self.map.brushes[j].clone())));
                // Highest index first so the lower one is still valid.
                pairs.sort_by_key(|p| std::cmp::Reverse(p.0));
                let cmds = pairs
                    .into_iter()
                    .map(|(i, b)| Box::new(RemoveBrush::new(i, b)) as Box<dyn Command>)
                    .collect();
                self.execute(Box::new(Batch::new("remove brush", cmds)));
            }
            Selection::Prop(i) => {
                let Some(p) = self.map.props.get(i).cloned() else {
                    return;
                };
                let twin = mirror
                    .prop(&p)
                    .and_then(|t| find_twin(&self.map.props, i, &t));
                let mut pairs = vec![(i, p)];
                pairs.extend(twin.map(|j| (j, self.map.props[j].clone())));
                pairs.sort_by_key(|p| std::cmp::Reverse(p.0));
                let cmds = pairs
                    .into_iter()
                    .map(|(i, p)| Box::new(RemoveProp::new(i, p)) as Box<dyn Command>)
                    .collect();
                self.execute(Box::new(Batch::new("remove prop", cmds)));
            }
        }
        self.selection = None;
    }

    fn mark(&mut self, touched: &[Aabb]) {
        for region in touched {
            self.meshes.mark_dirty(region);
        }
    }

    fn fix_selection(&mut self) {
        if self.selection.is_some_and(|s| s.get(&self.map).is_none()) {
            self.selection = None;
        }
    }

    /// Regenerates the mesh blocks touched since the last call and returns
    /// them.
    pub fn rebuild_meshes(&mut self) -> Vec<Block> {
        self.meshes.rebuild(&self.map)
    }

    // ---- selection ----

    /// The nearest brush or prop under the ray. Brushes are tested through
    /// their axis-aligned bounds (decision), props through their oriented
    /// bounds.
    #[must_use]
    pub fn pick(&self, ray: &Ray) -> Option<Selection> {
        let mut best: Option<(f32, Selection)> = None;
        let mut consider = |t: Option<f32>, sel: Selection| {
            if let Some(t) = t {
                if best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, sel));
                }
            }
        };
        for (i, b) in self.map.brushes.iter().enumerate() {
            consider(
                ray_aabb(ray.origin, ray.dir, &brush_aabb(b)),
                Selection::Brush(i),
            );
        }
        for (i, p) in self.map.props.iter().enumerate() {
            consider(ray_prop(ray, p, self.bounds.as_ref()), Selection::Prop(i));
        }
        best.map(|(_, s)| s)
    }

    /// World centre of the selection, where the handles sit.
    #[must_use]
    pub fn selection_centre(&self) -> Option<Vec3> {
        let element = self.selection?.get(&self.map)?;
        Some(element_centre(&element, self.bounds.as_ref()))
    }

    /// Length of the handles in metres for the current camera.
    #[must_use]
    pub fn handle_length(&self) -> Option<f32> {
        let centre = self.selection_centre()?;
        Some(handle_length(self.camera.distance_to(centre)))
    }

    // ---- pointer ----

    /// Whether a handle drag is in progress.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Primary button pressed under `ray`.
    pub fn pointer_press(&mut self, ray: &Ray) {
        match self.tool {
            Tool::Create | Tool::Zone => {
                if self.tool == Tool::Zone {
                    self.creation.settings.kind = crate::create::BrushKind::Box;
                }
                if let Some(brush) = self.creation.press(ray, &self.snap) {
                    self.finish_volume(brush);
                }
            }
            Tool::Prop | Tool::Spawn | Tool::Pickup | Tool::Light => {
                if let Some(point) = self.ground_point(ray) {
                    self.place_at(point);
                }
            }
            Tool::Select => {
                if self.begin_handle_drag(ray) {
                    return;
                }
                self.selection = self.pick(ray);
            }
            Tool::Sculpt => {
                if let Some(p) = self.terrain_point(ray) {
                    self.begin_stroke(p);
                }
            }
        }
    }

    // ---- terrain (lot 6.10) ----

    /// Where `ray` meets the terrain, if there is one under it.
    #[must_use]
    pub fn terrain_point(&self, ray: &Ray) -> Option<Vec3> {
        ray_terrain(self.map.terrain.as_ref()?, ray)
    }

    /// Adds a flat terrain covering the brushes (one command).
    pub fn add_terrain(&mut self) {
        if self.map.terrain.is_some() {
            return;
        }
        let t = terrain_for(&self.map);
        self.execute(Box::new(TerrainEdit::new("add terrain", None, Some(t))));
    }

    /// Removes the terrain (one command).
    pub fn remove_terrain(&mut self) {
        let old = self.map.terrain.clone();
        if old.is_some() {
            self.execute(Box::new(TerrainEdit::new("remove terrain", old, None)));
        }
    }

    /// The dab of the current settings at `p`, and at its mirror twin when a
    /// symmetry mode is on. Returns whether the terrain changed.
    fn dab_at(&mut self, p: Vec3, flatten_to: f32) -> bool {
        let settings = self.sculpt;
        let Some(t) = self.map.terrain.as_mut() else {
            return false;
        };
        let mut changed = dab(t, p.x, p.z, &settings, flatten_to);
        let twin = match self.symmetry {
            Symmetry::None => None,
            Symmetry::MirrorX => Some((-p.x, p.z)),
            Symmetry::MirrorZ => Some((p.x, -p.z)),
            Symmetry::Rotate180 => Some((-p.x, -p.z)),
        };
        if let Some((x, z)) = twin {
            changed |= dab(t, x, z, &settings, flatten_to);
        }
        changed
    }

    /// Starts a sculpt stroke at the terrain point `p` with one dab.
    pub fn begin_stroke(&mut self, p: Vec3) {
        self.cancel();
        let Some(before) = self.map.terrain.clone() else {
            return;
        };
        self.stroke = Some(Stroke {
            before,
            flatten_to: p.y,
        });
        self.stroke_to(p);
    }

    /// Adds a dab at `p` to the stroke in progress (the document shows it at
    /// once; the history records the stroke on [`Editor::end_stroke`]).
    pub fn stroke_to(&mut self, p: Vec3) {
        let Some(flatten_to) = self.stroke.as_ref().map(|s| s.flatten_to) else {
            return;
        };
        let before = self.map.terrain.clone();
        if self.dab_at(p, flatten_to) {
            if let (Some(a), Some(b)) = (before, self.map.terrain.clone()) {
                let preview = TerrainEdit::new("sculpt", Some(a), Some(b));
                let touched = preview.touched();
                self.mark(&touched);
            }
        }
    }

    /// Ends the stroke: the whole stroke becomes one history step.
    pub fn end_stroke(&mut self) {
        let Some(stroke) = self.stroke.take() else {
            return;
        };
        let after = self.map.terrain.clone();
        if after.as_ref() == Some(&stroke.before) {
            return;
        }
        let name = format!("sculpt ({})", self.sculpt.op.label());
        self.map.terrain = Some(stroke.before.clone());
        self.execute(Box::new(TerrainEdit::new(
            &name,
            Some(stroke.before),
            after,
        )));
    }

    /// One complete stroke of a single dab at `(x, z)` (on the terrain
    /// surface): a scripted sculpt, one history step.
    pub fn sculpt_at(&mut self, x: f32, z: f32) {
        let Some(y) = self.map.terrain.as_ref().and_then(|t| t.height_at(x, z)) else {
            return;
        };
        self.begin_stroke(Vec3::new(x, y, z));
        self.end_stroke();
    }

    /// Whether a sculpt stroke is in progress.
    #[must_use]
    pub fn is_sculpting(&self) -> bool {
        self.stroke.is_some()
    }

    /// The operation of the sculpt tool.
    pub fn set_sculpt_op(&mut self, op: SculptOp) {
        self.sculpt.op = op;
    }

    fn cancel_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            let current = self.map.terrain.clone();
            let back = TerrainEdit::new("cancel", current, Some(stroke.before.clone()));
            let touched = back.touched();
            self.map.terrain = Some(stroke.before);
            self.mark(&touched);
        }
    }

    fn begin_handle_drag(&mut self, ray: &Ray) -> bool {
        let (Some(sel), Some(length), Some(centre)) = (
            self.selection,
            self.handle_length(),
            self.selection_centre(),
        ) else {
            return false;
        };
        let (Some(original), Some(axis)) = (
            sel.get(&self.map),
            pick_handle(ray, centre, length, self.handle_mode),
        ) else {
            return false;
        };
        self.drag = Drag::begin(
            ray,
            sel,
            original,
            axis,
            self.handle_mode,
            length,
            self.bounds.as_ref(),
        );
        self.drag.is_some()
    }

    /// Pointer moved (with or without the button down).
    pub fn pointer_move(&mut self, ray: &Ray) {
        if let Some(mut drag) = self.drag.take() {
            let before = drag.current.clone();
            let after = drag.update(ray, &self.snap).clone();
            if after != before {
                self.write_preview(&drag, &before, after);
            }
            self.drag = Some(drag);
        } else if matches!(self.tool, Tool::Create | Tool::Zone) {
            self.creation.drag(ray, &self.snap);
        } else if self.stroke.is_some() {
            if let Some(p) = self.terrain_point(ray) {
                self.stroke_to(p);
            }
        }
    }

    /// Primary button released.
    pub fn pointer_release(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.finish_drag(&drag);
        } else if self.stroke.is_some() {
            self.end_stroke();
        } else if matches!(self.tool, Tool::Create | Tool::Zone) {
            let top = self.camera.mode == CameraMode::TopOrtho;
            if let Some(brush) = self.creation.release(top) {
                self.finish_volume(brush);
            }
        }
    }

    /// Abandons the gesture in progress (Escape): a handle drag is undone, a
    /// volume being drawn is dropped.
    pub fn cancel(&mut self) {
        self.cancel_drag();
        self.cancel_stroke();
        self.creation.cancel();
    }

    fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            let current = drag.current.clone();
            self.write_preview(&drag, &current, drag.original.clone());
        }
    }

    /// Shows `after` in the document while a drag is running, marking the
    /// blocks of both states dirty. The history is untouched until release.
    fn write_preview(&mut self, drag: &Drag, before: &Element, after: Element) {
        for e in [before, &after] {
            if let Element::Brush(b) = e {
                self.meshes.mark_dirty(&brush_aabb(b));
            }
        }
        drag.target.set(&mut self.map, after);
    }

    fn finish_drag(&mut self, drag: &Drag) {
        if drag.current == drag.original {
            return;
        }
        // Put the original back, then apply the edit through the history so
        // that undo restores it exactly.
        let current = drag.current.clone();
        self.write_preview(drag, &current, drag.original.clone());
        let mirror = self.mirror();
        match (drag.target, &drag.original, &drag.current) {
            (Selection::Brush(i), Element::Brush(old), Element::Brush(new)) => {
                let twin = mirror.brush(old).and_then(|t_old| {
                    let j = find_twin(&self.map.brushes, i, &t_old)?;
                    let t_new = mirror.brush(new)?;
                    Some(Box::new(ReplaceBrush::new(j, t_old, t_new)) as Box<dyn Command>)
                });
                let primary = Box::new(ReplaceBrush::new(i, old.clone(), new.clone()));
                self.execute_pair(primary, twin);
            }
            (Selection::Prop(i), Element::Prop(old), Element::Prop(new)) => {
                let twin = mirror.prop(old).and_then(|t_old| {
                    let j = find_twin(&self.map.props, i, &t_old)?;
                    let t_new = mirror.prop(new)?;
                    Some(Box::new(ReplaceProp::new(j, t_old, t_new)) as Box<dyn Command>)
                });
                let primary = Box::new(ReplaceProp::new(i, old.clone(), new.clone()));
                self.execute_pair(primary, twin);
            }
            _ => {}
        }
    }
}

/// Index of the element equal to `twin` other than the one at `own`.
pub(crate) fn find_twin<T: PartialEq>(list: &[T], own: usize, twin: &T) -> Option<usize> {
    list.iter()
        .enumerate()
        .find(|(j, e)| *j != own && *e == twin)
        .map(|(j, _)| j)
}
