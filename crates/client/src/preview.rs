//! The 3D character preview of the loadout screen (lot 6.3).
//!
//! It uses the renderer's existing interfaces only: unit cubes uploaded with
//! [`Renderer::upload_mesh`], one per colour, placed as [`Instance`]s and drawn
//! through a [`RenderWorld`]. The character is a stack of boxes (legs, torso,
//! head, arms) coloured from the palette, with a box for each carried weapon
//! sized by weapon id. The screen sits on the left of the display, so the
//! character is framed right of centre.

use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    cube_mesh, perspective_reverse_z, AmbienceProfile, Camera, Instance, MeshId, RenderError,
    RenderWorld, Renderer,
};
use faset_ui::{LoadoutSelection, PreviewRequest, PALETTE};

/// Seconds the character takes to turn once.
pub const TURN_SECONDS: f32 = 12.0;
/// Vertical field of view of the preview camera, degrees.
const FOV_Y_DEG: f32 = 38.0;

/// Index of the neutral colours after the palette in the mesh list.
const METAL: usize = PALETTE.len();
const SKIN: usize = PALETTE.len() + 1;
const GROUND: usize = PALETTE.len() + 2;

/// Linear colour of an sRGB byte triple (the vertex colour is linear).
fn linear(rgb: [u8; 3]) -> [u8; 3] {
    rgb.map(|c| ((f32::from(c) / 255.0).powf(2.2) * 255.0).round() as u8)
}

/// Meshes of the preview, uploaded once per renderer.
#[derive(Debug, Clone)]
pub struct CharacterPreview {
    /// One unit cube per palette colour, then metal, skin and ground.
    cubes: Vec<MeshId>,
}

/// Box size of a weapon held in the hand or carried (width, height, length).
fn weapon_size(id: &str) -> Vec3 {
    match id {
        "assault_rifle" => Vec3::new(0.07, 0.12, 0.85),
        "smg" => Vec3::new(0.07, 0.11, 0.55),
        "pump_shotgun" => Vec3::new(0.08, 0.1, 1.0),
        "precision_rifle" => Vec3::new(0.07, 0.1, 1.2),
        "boomer_rocket_launcher" => Vec3::new(0.18, 0.18, 1.0),
        "rubber_chicken_launcher" => Vec3::new(0.2, 0.2, 0.8),
        "sidearm" => Vec3::new(0.05, 0.12, 0.22),
        "baguette" => Vec3::new(0.07, 0.07, 0.8),
        "toaster" => Vec3::new(0.2, 0.16, 0.2),
        _ => Vec3::new(0.1, 0.1, 0.2),
    }
}

/// Width factor of a class's torso: the sturdy classes are broader.
fn bulk(class: &str) -> f32 {
    match class {
        "bruiser" => 1.25,
        "boomer" => 1.1,
        "marksman" | "sprinter" => 0.92,
        _ => 1.0,
    }
}

impl CharacterPreview {
    /// Uploads the cubes.
    ///
    /// # Errors
    /// The renderer refused a mesh (a full arena).
    pub fn upload(renderer: &mut Renderer) -> Result<Self, RenderError> {
        let colours = PALETTE.iter().map(|c| linear(c.rgb)).chain([
            linear([70, 74, 82]),
            linear([226, 184, 150]),
            linear([40, 44, 40]),
        ]);
        let mut cubes = Vec::new();
        for rgb in colours {
            let (vertices, indices) = cube_mesh(0.5, &[rgb; 6], 0);
            cubes.push(renderer.upload_mesh(&vertices, &indices)?);
        }
        Ok(Self { cubes })
    }

    fn part(&self, colour: usize, centre: Vec3, size: Vec3, yaw: Mat4) -> Instance {
        let world = yaw * Mat4::from_translation(centre) * Mat4::from_scale(size);
        Instance::new(self.cubes[colour], 0, world)
    }

    /// The instances of the character turned by `yaw` radians.
    #[must_use]
    pub fn instances(&self, selection: &LoadoutSelection, yaw: f32) -> Vec<Instance> {
        let turn = Mat4::from_rotation_y(yaw);
        let last = PALETTE.len() - 1;
        let body = usize::from(selection.body_colour).min(last);
        let accent = usize::from(selection.accent_colour).min(last);
        let b = bulk(&selection.class);
        let mut out = vec![
            self.part(
                GROUND,
                Vec3::new(0.0, -0.025, 0.0),
                Vec3::new(2.4, 0.05, 2.4),
                Mat4::IDENTITY,
            ),
            // Legs, torso, head, arms.
            self.part(
                accent,
                Vec3::new(-0.14 * b, 0.425, 0.0),
                Vec3::new(0.22 * b, 0.85, 0.26),
                turn,
            ),
            self.part(
                accent,
                Vec3::new(0.14 * b, 0.425, 0.0),
                Vec3::new(0.22 * b, 0.85, 0.26),
                turn,
            ),
            self.part(
                body,
                Vec3::new(0.0, 1.17, 0.0),
                Vec3::new(0.56 * b, 0.64, 0.3),
                turn,
            ),
            self.part(SKIN, Vec3::new(0.0, 1.65, 0.0), Vec3::splat(0.27), turn),
            self.part(
                accent,
                Vec3::new(0.0, 1.69, -0.12),
                Vec3::new(0.29, 0.08, 0.06),
                turn,
            ),
            self.part(
                body,
                Vec3::new(-0.36 * b, 1.15, -0.05),
                Vec3::new(0.14, 0.58, 0.16),
                turn,
            ),
            self.part(
                body,
                Vec3::new(0.36 * b, 1.2, -0.2),
                Vec3::new(0.14, 0.14, 0.5),
                turn,
            ),
        ];
        let primary = weapon_size(&selection.primary);
        out.push(self.part(
            METAL,
            Vec3::new(0.3 * b, 1.2, -0.3 - primary.z * 0.3),
            primary,
            turn,
        ));
        if let Some(id) = &selection.secondary {
            out.push(self.part(
                METAL,
                Vec3::new(0.32 * b, 0.82, -0.05),
                weapon_size(id),
                turn,
            ));
        }
        if let Some(id) = &selection.melee {
            // Slung across the back, upright.
            let s = weapon_size(id);
            out.push(self.part(
                accent,
                Vec3::new(-0.15, 1.2, 0.22),
                Vec3::new(s.x, s.z, s.y),
                turn,
            ));
        }
        if let Some(id) = &selection.gadget {
            out.push(self.part(
                accent,
                Vec3::new(-0.3 * b, 0.9, -0.1),
                weapon_size(id),
                turn,
            ));
        }
        out
    }

    /// The camera that frames the character, for a display of `aspect`.
    #[must_use]
    pub fn camera(aspect: f32) -> Camera {
        let view = faset_render::glam::camera::rh::view::look_at_mat4(
            Vec3::new(-0.6, 1.35, 3.6),
            Vec3::new(-0.6, 0.95, 0.0),
            Vec3::Y,
        );
        Camera::new(
            view,
            perspective_reverse_z(FOV_Y_DEG.to_radians(), aspect, 0.1),
        )
    }

    /// A frame showing the preview request.
    #[must_use]
    pub fn render_world(&self, request: &PreviewRequest, yaw: f32, aspect: f32) -> RenderWorld {
        RenderWorld {
            clear_color: [0.02, 0.025, 0.035, 1.0],
            camera: Self::camera(aspect),
            instances: self.instances(&request.selection, yaw),
            ambience: AmbienceProfile::builtin("day").unwrap_or_default(),
            ..RenderWorld::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_conversion_keeps_the_ends() {
        assert_eq!(linear([0, 0, 0]), [0, 0, 0]);
        assert_eq!(linear([255, 255, 255]), [255, 255, 255]);
        assert!(linear([128, 128, 128])[0] < 128);
    }

    #[test]
    fn weapon_sizes_and_bulk_have_defaults() {
        assert!(weapon_size("assault_rifle").z > weapon_size("sidearm").z);
        assert_eq!(weapon_size("unknown"), Vec3::new(0.1, 0.1, 0.2));
        assert!(bulk("bruiser") > bulk("trooper"));
    }
}
