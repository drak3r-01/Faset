//! Undoable modifications of the map document.
//!
//! Every edit is a [`Command`]. A command applies itself to the
//! [`MapFile`] and reverts exactly, so that after an undo the serialised map
//! is byte-equal to what it was before the command.

use faset_mapfmt::{Brush, MapFile, Prop};

use crate::element::brush_aabb;
use crate::math::Aabb;

/// One reversible edit.
pub trait Command {
    /// Short label for the history list.
    fn name(&self) -> &str;
    /// Applies the edit. Called again after a revert for a redo.
    fn apply(&mut self, map: &mut MapFile);
    /// Undoes [`Command::apply`] exactly.
    fn revert(&mut self, map: &mut MapFile);
    /// World regions whose geometry differs between the states before and
    /// after the command: the mesh blocks overlapping them are regenerated.
    fn touched(&self) -> Vec<Aabb>;
}

/// A stack of applied commands, with the undone ones kept for redo.
#[derive(Default)]
pub struct History {
    done: Vec<Box<dyn Command>>,
    undone: Vec<Box<dyn Command>>,
}

impl History {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies `command` to `map` and records it; drops the redo stack.
    /// Returns the regions the command touched.
    pub fn push(&mut self, mut command: Box<dyn Command>, map: &mut MapFile) -> Vec<Aabb> {
        command.apply(map);
        let touched = command.touched();
        self.done.push(command);
        self.undone.clear();
        touched
    }

    /// Reverts the last command. `None` when there is nothing to undo.
    pub fn undo(&mut self, map: &mut MapFile) -> Option<Vec<Aabb>> {
        let mut command = self.done.pop()?;
        command.revert(map);
        let touched = command.touched();
        self.undone.push(command);
        Some(touched)
    }

    /// Re-applies the last undone command. `None` when there is none.
    pub fn redo(&mut self, map: &mut MapFile) -> Option<Vec<Aabb>> {
        let mut command = self.undone.pop()?;
        command.apply(map);
        let touched = command.touched();
        self.done.push(command);
        Some(touched)
    }

    /// Number of commands that can be undone.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.done.len()
    }

    /// Number of commands that can be redone.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.undone.len()
    }

    /// Name of the command an undo would revert.
    #[must_use]
    pub fn next_undo(&self) -> Option<&str> {
        self.done.last().map(|c| c.name())
    }

    /// Forgets every command (after loading another map).
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }
}

/// Appends a brush.
pub struct AddBrush {
    brush: Brush,
}

impl AddBrush {
    #[must_use]
    pub fn new(brush: Brush) -> Self {
        Self { brush }
    }
}

impl Command for AddBrush {
    fn name(&self) -> &str {
        "add brush"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.brushes.push(self.brush.clone());
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.brushes.pop();
    }
    fn touched(&self) -> Vec<Aabb> {
        vec![brush_aabb(&self.brush)]
    }
}

/// Removes the brush at `index`; `brush` is the removed value, kept to put
/// it back at the same place.
pub struct RemoveBrush {
    index: usize,
    brush: Brush,
}

impl RemoveBrush {
    #[must_use]
    pub fn new(index: usize, brush: Brush) -> Self {
        Self { index, brush }
    }
}

impl Command for RemoveBrush {
    fn name(&self) -> &str {
        "remove brush"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.brushes.remove(self.index);
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.brushes.insert(self.index, self.brush.clone());
    }
    fn touched(&self) -> Vec<Aabb> {
        vec![brush_aabb(&self.brush)]
    }
}

/// Replaces the brush at `index` (move, rotate, scale, material change).
pub struct ReplaceBrush {
    index: usize,
    old: Brush,
    new: Brush,
}

impl ReplaceBrush {
    #[must_use]
    pub fn new(index: usize, old: Brush, new: Brush) -> Self {
        Self { index, old, new }
    }
}

impl Command for ReplaceBrush {
    fn name(&self) -> &str {
        "edit brush"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.brushes[self.index] = self.new.clone();
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.brushes[self.index] = self.old.clone();
    }
    fn touched(&self) -> Vec<Aabb> {
        vec![brush_aabb(&self.old), brush_aabb(&self.new)]
    }
}

/// Appends a prop.
pub struct AddProp {
    prop: Prop,
}

impl AddProp {
    #[must_use]
    pub fn new(prop: Prop) -> Self {
        Self { prop }
    }
}

impl Command for AddProp {
    fn name(&self) -> &str {
        "add prop"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.props.push(self.prop.clone());
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.props.pop();
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}

/// Removes the prop at `index`.
pub struct RemoveProp {
    index: usize,
    prop: Prop,
}

impl RemoveProp {
    #[must_use]
    pub fn new(index: usize, prop: Prop) -> Self {
        Self { index, prop }
    }
}

impl Command for RemoveProp {
    fn name(&self) -> &str {
        "remove prop"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.props.remove(self.index);
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.props.insert(self.index, self.prop.clone());
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}

/// Replaces the prop at `index` (move, rotate, scale).
pub struct ReplaceProp {
    index: usize,
    old: Prop,
    new: Prop,
}

impl ReplaceProp {
    #[must_use]
    pub fn new(index: usize, old: Prop, new: Prop) -> Self {
        Self { index, old, new }
    }
}

impl Command for ReplaceProp {
    fn name(&self) -> &str {
        "edit prop"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.props[self.index] = self.new.clone();
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.props[self.index] = self.old.clone();
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}
