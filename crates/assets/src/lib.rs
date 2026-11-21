//! Mesh recipes: every prop is a RON [`Recipe`] turned into a flat-shaded
//! triangle [`Mesh`] at load time; no model file is ever imported.
//!
//! Colours are palette names resolved from a [`Palette`] at build time.
//! Building is deterministic: the same recipe, palette and seed give the
//! same mesh. Units are metres, Y up, -Z forward.

pub mod anim;
mod build;
pub mod character;
mod error;
mod mesh;
mod palette;
mod prims;
mod recipe;
mod validate;
pub mod viewmodel;

pub use build::build_mesh;
pub use error::{BuildError, RecipeError};
pub use mesh::{Mesh, Vertex};
pub use palette::Palette;
pub use recipe::{Axis, Collision, Part, Pattern, Recipe};
pub use validate::{validate, AssetKind, Validation, MAP_TRIANGLE_BUDGET};
