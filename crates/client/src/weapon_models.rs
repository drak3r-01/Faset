//! Weapon models (lot A, art direction): the recipes of `assets/weapons`
//! are built once with `faset-assets`, uploaded to the renderer and drawn for
//! the weapon in the local player's hands and for the weapons the other
//! characters carry.
//!
//! The recipes are embedded in the executable. A weapon id without a recipe
//! has no model and the caller falls back to a plain box.

use std::collections::HashMap;

use faset_assets::{build_mesh, Palette, Recipe};
use faset_render::glam::Vec3;
use faset_render::{MeshId, RenderError, Renderer, Vertex};

/// Embedded recipes, by weapon id.
const RECIPES: [(&str, &str); 12] = [
    (
        "assault_rifle",
        include_str!("../../../assets/weapons/assault_rifle.ron"),
    ),
    ("smg", include_str!("../../../assets/weapons/smg.ron")),
    (
        "pump_shotgun",
        include_str!("../../../assets/weapons/pump_shotgun.ron"),
    ),
    (
        "precision_rifle",
        include_str!("../../../assets/weapons/precision_rifle.ron"),
    ),
    (
        "sidearm",
        include_str!("../../../assets/weapons/sidearm.ron"),
    ),
    (
        "baguette",
        include_str!("../../../assets/weapons/baguette.ron"),
    ),
    (
        "toaster",
        include_str!("../../../assets/weapons/toaster.ron"),
    ),
    (
        "banana_peel",
        include_str!("../../../assets/weapons/banana_peel.ron"),
    ),
    (
        "bubble_gun",
        include_str!("../../../assets/weapons/bubble_gun.ron"),
    ),
    (
        "confetti_cannon",
        include_str!("../../../assets/weapons/confetti_cannon.ron"),
    ),
    (
        "rubber_chicken_launcher",
        include_str!("../../../assets/weapons/rubber_chicken_launcher.ron"),
    ),
    (
        "boomer_rocket_launcher",
        include_str!("../../../assets/weapons/boomer_rocket_launcher.ron"),
    ),
];

/// One uploaded weapon model.
#[derive(Debug, Clone, Copy)]
pub struct WeaponModel {
    pub mesh: MeshId,
    /// Length from the grip (the origin) to the muzzle, metres.
    pub length: f32,
}

/// The uploaded weapon models.
#[derive(Debug, Clone, Default)]
pub struct WeaponModels {
    models: HashMap<String, WeaponModel>,
}

impl WeaponModels {
    /// Builds and uploads every embedded recipe. A recipe that does not build
    /// is skipped with a warning (the weapon then draws as a box).
    ///
    /// # Errors
    /// The renderer refused a mesh (a full arena).
    pub fn upload(renderer: &mut Renderer) -> Result<Self, RenderError> {
        let palette = Palette::default_theme();
        let mut models = HashMap::new();
        for (id, text) in RECIPES {
            let mesh = match Recipe::from_ron(text)
                .map_err(|e| e.to_string())
                .and_then(|r| build_mesh(&r, &palette, None).map_err(|e| e.to_string()))
            {
                Ok(mesh) => mesh,
                Err(error) => {
                    tracing::warn!(weapon = id, %error, "weapon model skipped");
                    continue;
                }
            };
            let length = mesh.bounds().map_or(0.5, |(lo, _)| (-lo.z).max(0.2));
            let vertices: Vec<Vertex> = mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color_material: v.color_material,
                })
                .collect();
            let mesh = renderer.upload_mesh(&vertices, &mesh.indices)?;
            models.insert(id.to_owned(), WeaponModel { mesh, length });
        }
        Ok(Self { models })
    }

    /// The model of a weapon id, if it has one.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<WeaponModel> {
        self.models.get(id).copied()
    }

    /// Number of models.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// Where the grip of a held weapon sits relative to the camera, metres
/// (right, down, forward is -Z) for a weapon of the given `length`.
#[must_use]
pub fn view_grip(side: f32, length: f32) -> Vec3 {
    // Longer weapons sit slightly lower and further out so the muzzle stays
    // in the frame without filling the screen.
    Vec3::new(side * 0.85, -0.2 - 0.03 * length.min(1.2), -0.36)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_recipe_builds() {
        let palette = Palette::default_theme();
        for (id, text) in RECIPES {
            let recipe = Recipe::from_ron(text).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(recipe.id, id);
            let mesh = build_mesh(&recipe, &palette, None).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(mesh.triangle_count() >= 150, "{id} is too plain");
            let (lo, hi) = mesh.bounds().unwrap();
            assert!(lo.z < -0.15, "{id}: the muzzle points to -Z");
            assert!(hi.z < 0.6, "{id}: the grip is near the origin");
        }
    }

    #[test]
    fn the_grip_stays_below_the_eye_line() {
        assert!(view_grip(0.2, 1.0).y < -0.1);
        assert!(view_grip(0.2, 1.0).z < 0.0);
    }
}
