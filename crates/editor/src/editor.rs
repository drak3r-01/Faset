//! The editor state: document, history, camera, selection and tools.

use faset_core::glam::Vec3;
use faset_mapfmt::{Brush, MapFile, Meta, Prop, Symmetry, Theme};

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
use crate::snap::SnapSettings;

/// The active tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Pick elements and drag their handles.
    #[default]
    Select,
    /// Draw new volumes.
    Create,
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
    /// Prop bounds used for picking; replaced by the library in lot 5.3.
    pub bounds: Box<dyn PropBounds>,
    drag: Option<Drag>,
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
            drag: None,
        }
    }

    /// An editor over a new empty map.
    #[must_use]
    pub fn blank(name: &str) -> Self {
        Self::new(new_map(name))
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
        let Some(touched) = self.history.redo(&mut self.map) else {
            return false;
        };
        self.mark(&touched);
        self.fix_selection();
        true
    }

    /// Adds a brush through a command and selects it.
    pub fn add_brush(&mut self, brush: Brush) {
        self.execute(Box::new(AddBrush::new(brush)));
        self.selection = Some(Selection::Brush(self.map.brushes.len() - 1));
    }

    /// Adds a prop through a command and selects it.
    pub fn add_prop(&mut self, prop: Prop) {
        self.execute(Box::new(AddProp::new(prop)));
        self.selection = Some(Selection::Prop(self.map.props.len() - 1));
    }

    /// Deletes the selected element through a command.
    pub fn delete_selection(&mut self) {
        let Some(sel) = self.selection else { return };
        match sel.get(&self.map) {
            Some(Element::Brush(b)) => {
                let Selection::Brush(i) = sel else { return };
                self.execute(Box::new(RemoveBrush::new(i, b)));
            }
            Some(Element::Prop(p)) => {
                let Selection::Prop(i) = sel else { return };
                self.execute(Box::new(RemoveProp::new(i, p)));
            }
            None => {}
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
            Tool::Create => {
                if let Some(brush) = self.creation.press(ray, &self.snap) {
                    self.add_brush(brush);
                }
            }
            Tool::Select => {
                if self.begin_handle_drag(ray) {
                    return;
                }
                self.selection = self.pick(ray);
            }
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
        } else if self.tool == Tool::Create {
            self.creation.drag(ray, &self.snap);
        }
    }

    /// Primary button released.
    pub fn pointer_release(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.finish_drag(&drag);
        } else if self.tool == Tool::Create {
            let top = self.camera.mode == CameraMode::TopOrtho;
            if let Some(brush) = self.creation.release(top) {
                self.add_brush(brush);
            }
        }
    }

    /// Abandons the gesture in progress (Escape): a handle drag is undone, a
    /// volume being drawn is dropped.
    pub fn cancel(&mut self) {
        self.cancel_drag();
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
        let command: Box<dyn Command> = match (drag.target, &drag.original, &drag.current) {
            (Selection::Brush(i), Element::Brush(old), Element::Brush(new)) => {
                Box::new(ReplaceBrush::new(i, old.clone(), new.clone()))
            }
            (Selection::Prop(i), Element::Prop(old), Element::Prop(new)) => {
                Box::new(ReplaceProp::new(i, old.clone(), new.clone()))
            }
            _ => return,
        };
        self.execute(command);
    }
}
