//! In-game map editor, core.
//!
//! The [`Editor`] (alias [`EditorState`]) is a client state whose document is
//! directly the [`faset_mapfmt::MapFile`]: what is edited is what is saved.
//! Every modification is a [`Command`] with `apply` and `revert`, pushed on a
//! [`History`]; undo reverts it exactly, so after each undo the serialised
//! RON is byte-equal to what it was before the command.
//!
//! - [`EditorCamera`]: free fly and orthographic top view, producing the
//!   render crate's [`faset_render::Camera`] and mouse rays;
//! - selection: a ray against volumes (axis-aligned bounds) and props
//!   (oriented bounds);
//! - handles ([`HandleMode`]): move, rotate, scale with snapping to the
//!   0.5 m grid and to 15 degree angles; axis drags use the closest point
//!   between the handle axis and the mouse ray ([`axis_drag_param`]);
//! - volume creation ([`Creation`]): drag a rectangle on the ground, then
//!   pull the height; boxes, wedges, cylinders, prisms and stairs;
//! - [`MeshCache`]: only the 16 m blocks touched by a command are rebuilt;
//! - an egui panel and input handling ([`faset_ui::Screen`] for [`Editor`]).
//!
//! Lot 5.3 adds the [`Library`] (props with CPU-rendered thumbnails,
//! materials, palettes), the placement tools for props, prefab instances,
//! spawns, pickups, zones and lights, the map info, theme and physics panels,
//! and the symmetry modes: with a [`Symmetry`](faset_mapfmt::Symmetry) on,
//! every placement, deletion and handle edit also applies to its mirror twin
//! in the same history step (see [`Mirror`]).
//!
//! Lot 5.4 adds continuous validation ([`ValidationPanel`]: the `map-tool`
//! rules re-run after each edit, a click on an issue frames it with the
//! camera), the immediate test ([`TestSession`]: an integrated server on the
//! document, the player at the camera) and files ([`FileService`]: save to
//! `maps/`, export, import, autosave every 60 s to a separate folder).
//!
//! Lot 6.10 adds terrain sculpting ([`Tool::Sculpt`], [`SculptSettings`]):
//! raise, lower, smooth, flatten and paint the map's height field with a
//! round brush; a stroke is one [`TerrainEdit`] in the history, mirrored
//! with the symmetry mode, and the terrain is saved in the map file.
#![forbid(unsafe_code)]

mod camera;
mod command;
mod create;
mod edit;
mod editor;
mod element;
mod files;
mod gameplay;
mod handles;
mod library;
mod math;
mod mesh;
mod panels;
mod sculpt;
mod snap;
mod symmetry;
mod test_session;
mod ui;
mod validation;
mod workflow;

pub use camera::{
    orthographic_reverse_z, ray_from_ndc, CameraInput, CameraMode, EditorCamera, FOCUS_DISTANCE,
};
pub use command::{
    AddBrush, AddProp, Command, History, RemoveBrush, RemoveProp, ReplaceBrush, ReplaceProp,
};
pub use create::{make_brush, BrushKind, CreateStage, Creation, CreationSettings};
pub use edit::{
    AddItem, AddPrefab, Batch, Item, PlacePrefab, RemoveItem, ReplaceItem, SetMeta, SetPhysics,
    SetTheme,
};
pub use editor::{new_map, Editor, EditorState, Tool};
pub use element::{
    brush_aabb, brush_extent, brush_grid_bounds, element_centre, prop_rotation, ray_prop,
    resize_brush, rotate_brush, translate_brush, Element, PropBounds, Selection, UnitPropBounds,
};
pub use files::{FileError, FileService, Imported, AUTOSAVE_INTERVAL_S};
pub use gameplay::{PlaceSettings, ThemePreview};
pub use handles::{
    handle_length, pick_handle, Drag, HandleAxis, HandleMode, HANDLE_SCREEN_FRACTION,
};
pub use library::{
    render_thumbnail, Library, LibraryBounds, LibraryError, PropEntry, Thumbnail, AMBIENCES,
    MATERIALS, PALETTES, SKIES, THUMBNAIL_SIZE, WEATHERS,
};
pub use math::{axis_drag_param, ray_aabb, ray_horizontal_plane, Aabb, Ray};
pub use mesh::{Block, MeshCache};
pub use panels::{LibraryTab, PanelState};
pub use sculpt::{dab, ray_terrain, terrain_for, SculptOp, SculptSettings, TerrainEdit};
pub use snap::SnapSettings;
pub use symmetry::Mirror;
pub use test_session::{TestError, TestSession};
pub use validation::{validate_bytes, Severity, ValidationIssue, ValidationPanel, DEBOUNCE_S};
pub use workflow::WorkflowUi;
