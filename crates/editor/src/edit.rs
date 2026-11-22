//! More undoable commands: gameplay elements, document blocks, prefabs and
//! the [`Batch`] that makes an edit and its symmetric twin one history step.

use faset_mapfmt::{Light, MapFile, Meta, Physics, Pickup, Placement, Prefab, Spawn, Theme, Zone};

use crate::command::Command;
use crate::element::brush_aabb;
use crate::math::Aabb;

/// Several commands applied in order and reverted in reverse order: one
/// history step (used for an edit plus its mirror twin).
pub struct Batch {
    name: String,
    commands: Vec<Box<dyn Command>>,
}

impl Batch {
    #[must_use]
    pub fn new(name: &str, commands: Vec<Box<dyn Command>>) -> Self {
        Self {
            name: name.to_owned(),
            commands,
        }
    }
}

impl Command for Batch {
    fn name(&self) -> &str {
        &self.name
    }
    fn apply(&mut self, map: &mut MapFile) {
        for c in &mut self.commands {
            c.apply(map);
        }
    }
    fn revert(&mut self, map: &mut MapFile) {
        for c in self.commands.iter_mut().rev() {
            c.revert(map);
        }
    }
    fn touched(&self) -> Vec<Aabb> {
        self.commands.iter().flat_map(|c| c.touched()).collect()
    }
}

/// A gameplay element stored in one of the document lists.
pub trait Item: Clone + PartialEq {
    /// Label of the kind, for history names.
    const LABEL: &'static str;
    /// The list of the document holding this kind of element.
    fn list(map: &mut MapFile) -> &mut Vec<Self>;
}

macro_rules! item {
    ($ty:ty, $field:ident, $label:literal) => {
        impl Item for $ty {
            const LABEL: &'static str = $label;
            fn list(map: &mut MapFile) -> &mut Vec<Self> {
                &mut map.$field
            }
        }
    };
}
item!(Spawn, spawns, "spawn");
item!(Pickup, pickups, "pickup");
item!(Zone, zones, "zone");
item!(Light, lights, "light");

/// Appends an element to its list.
pub struct AddItem<T: Item> {
    item: T,
    name: String,
}

impl<T: Item> AddItem<T> {
    #[must_use]
    pub fn new(item: T) -> Self {
        Self {
            item,
            name: format!("add {}", T::LABEL),
        }
    }
}

impl<T: Item> Command for AddItem<T> {
    fn name(&self) -> &str {
        &self.name
    }
    fn apply(&mut self, map: &mut MapFile) {
        T::list(map).push(self.item.clone());
    }
    fn revert(&mut self, map: &mut MapFile) {
        T::list(map).pop();
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}

/// Removes the element at `index`.
pub struct RemoveItem<T: Item> {
    index: usize,
    item: T,
    name: String,
}

impl<T: Item> RemoveItem<T> {
    #[must_use]
    pub fn new(index: usize, item: T) -> Self {
        Self {
            index,
            item,
            name: format!("remove {}", T::LABEL),
        }
    }
}

impl<T: Item> Command for RemoveItem<T> {
    fn name(&self) -> &str {
        &self.name
    }
    fn apply(&mut self, map: &mut MapFile) {
        T::list(map).remove(self.index);
    }
    fn revert(&mut self, map: &mut MapFile) {
        T::list(map).insert(self.index, self.item.clone());
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}

/// Replaces the element at `index`.
pub struct ReplaceItem<T: Item> {
    index: usize,
    old: T,
    new: T,
    name: String,
}

impl<T: Item> ReplaceItem<T> {
    #[must_use]
    pub fn new(index: usize, old: T, new: T) -> Self {
        Self {
            index,
            old,
            new,
            name: format!("edit {}", T::LABEL),
        }
    }
}

impl<T: Item> Command for ReplaceItem<T> {
    fn name(&self) -> &str {
        &self.name
    }
    fn apply(&mut self, map: &mut MapFile) {
        T::list(map)[self.index] = self.new.clone();
    }
    fn revert(&mut self, map: &mut MapFile) {
        T::list(map)[self.index] = self.old.clone();
    }
    fn touched(&self) -> Vec<Aabb> {
        Vec::new()
    }
}

macro_rules! block_command {
    ($name:ident, $ty:ty, $field:ident, $label:literal) => {
        /// Replaces a whole block of the document.
        pub struct $name {
            old: $ty,
            new: $ty,
        }

        impl $name {
            #[must_use]
            pub fn new(old: $ty, new: $ty) -> Self {
                Self { old, new }
            }
        }

        impl Command for $name {
            fn name(&self) -> &str {
                $label
            }
            fn apply(&mut self, map: &mut MapFile) {
                map.$field.clone_from(&self.new);
            }
            fn revert(&mut self, map: &mut MapFile) {
                map.$field.clone_from(&self.old);
            }
            fn touched(&self) -> Vec<Aabb> {
                Vec::new()
            }
        }
    };
}
block_command!(SetMeta, Meta, meta, "edit map info");
block_command!(SetTheme, Theme, theme, "edit theme");
block_command!(SetPhysics, Option<Physics>, physics, "edit physics");

/// The regions a prefab placement fills, in world metres.
fn placement_region(prefab: &Prefab, place: Placement) -> Vec<Aabb> {
    let single = Prefab {
        place: vec![place],
        ..prefab.clone()
    };
    let mut map = crate::editor::new_map("region");
    map.prefabs.push(single);
    faset_mapfmt::expand_prefabs(&map)
        .brushes
        .iter()
        .map(brush_aabb)
        .collect()
}

/// Appends a prefab definition (with its placements, usually none yet).
pub struct AddPrefab {
    prefab: Prefab,
}

impl AddPrefab {
    #[must_use]
    pub fn new(prefab: Prefab) -> Self {
        Self { prefab }
    }
}

impl Command for AddPrefab {
    fn name(&self) -> &str {
        "define prefab"
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.prefabs.push(self.prefab.clone());
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.prefabs.pop();
    }
    fn touched(&self) -> Vec<Aabb> {
        self.prefab
            .place
            .iter()
            .flat_map(|p| placement_region(&self.prefab, *p))
            .collect()
    }
}

/// Adds one instance of a prefab: the definition is not copied, the instance
/// only references it by name.
pub struct PlacePrefab {
    prefab: String,
    place: Placement,
    region: Vec<Aabb>,
}

impl PlacePrefab {
    /// `definition` is the prefab being instanced (used for dirty regions).
    #[must_use]
    pub fn new(definition: &Prefab, place: Placement) -> Self {
        Self {
            prefab: definition.name.clone(),
            place,
            region: placement_region(definition, place),
        }
    }
}

impl Command for PlacePrefab {
    fn name(&self) -> &str {
        "place prefab"
    }
    fn apply(&mut self, map: &mut MapFile) {
        if let Some(p) = map.prefabs.iter_mut().find(|p| p.name == self.prefab) {
            p.place.push(self.place);
        }
    }
    fn revert(&mut self, map: &mut MapFile) {
        if let Some(p) = map.prefabs.iter_mut().find(|p| p.name == self.prefab) {
            p.place.pop();
        }
    }
    fn touched(&self) -> Vec<Aabb> {
        self.region.clone()
    }
}
