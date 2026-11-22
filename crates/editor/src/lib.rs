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
//! Library, prop placement UI, gameplay points, ambience and symmetry belong
//! to lot 5.3; validation, test launch and file I/O to lot 5.4.
#![forbid(unsafe_code)]

mod camera;
mod command;
mod create;
mod editor;
mod element;
mod handles;
mod math;
mod mesh;
mod snap;
mod ui;

pub use camera::{orthographic_reverse_z, ray_from_ndc, CameraInput, CameraMode, EditorCamera};
pub use command::{
    AddBrush, AddProp, Command, History, RemoveBrush, RemoveProp, ReplaceBrush, ReplaceProp,
};
pub use create::{make_brush, BrushKind, CreateStage, Creation, CreationSettings};
pub use editor::{new_map, Editor, EditorState, Tool};
pub use element::{
    brush_aabb, brush_extent, brush_grid_bounds, element_centre, prop_rotation, ray_prop,
    resize_brush, rotate_brush, translate_brush, Element, PropBounds, Selection, UnitPropBounds,
};
pub use handles::{
    handle_length, pick_handle, Drag, HandleAxis, HandleMode, HANDLE_SCREEN_FRACTION,
};
pub use math::{axis_drag_param, ray_aabb, ray_horizontal_plane, Aabb, Ray};
pub use mesh::{Block, MeshCache};
pub use snap::SnapSettings;
