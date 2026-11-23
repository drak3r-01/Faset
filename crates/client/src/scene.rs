//! What a match shows besides the map (lot 6.6): the other players, the
//! pickups, the projectiles and blasts, and the weapon in the local
//! player's hands.
//!
//! Everything is built from unit boxes through [`CharacterPreview`]'s
//! cubes, the same way the loadout screen draws its character, so no new
//! renderer interface is needed. The weapon in hand is a normal instance
//! placed in front of the camera and animated by the springs of
//! `faset-assets` (sway, step, recoil); the renderer has no separate
//! first-person pass yet, so the weapon can sink into a wall the player
//! stands against.

use faset_assets::viewmodel::{RecoilPattern, Viewmodel, ViewmodelInputs};
use faset_core::glam::Vec3;
use faset_core::settings::WeaponPosition;
use faset_mode_fps::PickupKind;
use faset_render::glam::{Mat4, Quat};
use faset_render::{Instance, RenderError, Renderer};
use faset_ui::LoadoutSelection;

use crate::camera::Look;
use crate::preview::{weapon_size, CharacterPreview, METAL, SKIN};
use crate::session::Session;

/// Height of the capsule centre above the feet, metres.
const CENTRE_HEIGHT: f32 = 0.9;
/// Palette indices used for teams and pickups (see [`faset_ui::PALETTE`]).
const SLATE: usize = 0;
const CRIMSON: usize = 1;
const AMBER: usize = 2;
const LIME: usize = 3;
const TEAL: usize = 4;
const AZURE: usize = 5;
const VIOLET: usize = 6;
const BONE: usize = 7;
/// Scale of the weapon in the first-person view.
const VIEW_SCALE: f32 = 0.6;

/// What the scene needs of the local player each frame.
#[derive(Debug, Clone, Copy)]
pub struct SceneFrame {
    /// Eye position and orientation of the camera.
    pub eye: Vec3,
    pub orientation: Quat,
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Horizontal speed, m/s.
    pub speed: f32,
    pub grounded: bool,
    /// Where the weapon is held.
    pub weapon_position: WeaponPosition,
    /// The look angles, for the weapon's sway.
    pub look: Look,
}

/// The meshes and animation state of a match's scene.
pub struct MatchScene {
    cubes: CharacterPreview,
    viewmodel: Viewmodel,
    pattern: RecoilPattern,
    prev_look: Look,
    phase: f32,
}

/// Palette colour of a pickup.
fn pickup_colour(kind: PickupKind) -> usize {
    match kind {
        PickupKind::Heal => LIME,
        PickupKind::Ammo => AMBER,
        PickupKind::Armour => AZURE,
        PickupKind::Speed => TEAL,
        PickupKind::Damage => CRIMSON,
        PickupKind::SuperJump => VIOLET,
        PickupKind::BigHead => BONE,
    }
}

/// Body colour of a player: the team's, or one of five for free-for-all.
fn body_colour(team: Option<u8>, id: u16) -> u8 {
    match team {
        Some(0) => AZURE as u8,
        Some(_) => CRIMSON as u8,
        None => [CRIMSON, AMBER, LIME, TEAL, VIOLET][usize::from(id) % 5] as u8,
    }
}

impl MatchScene {
    /// Uploads the boxes.
    ///
    /// # Errors
    /// The renderer refused a mesh.
    pub fn upload(renderer: &mut Renderer) -> Result<Self, RenderError> {
        Ok(Self {
            cubes: CharacterPreview::upload(renderer)?,
            viewmodel: Viewmodel::new(1),
            pattern: RecoilPattern::test_rifle(),
            prev_look: Look::default(),
            phase: 0.0,
        })
    }

    /// The instances of the scene at the session's present: other players,
    /// pickups, projectiles, blasts and the local weapon.
    pub fn instances(&mut self, session: &mut Session, frame: &SceneFrame) -> Vec<Instance> {
        let now = session.time();
        let mut out = Vec::new();

        // The other players, interpolated in the past.
        let render = session
            .remotes()
            .render_time(now, session.clock().jitter())
            .unwrap_or(0.0);
        let remotes: Vec<_> = session.remotes().sample_all(render).collect();
        let me = session.player();
        let info = session.view_mut().players();
        for (id, view) in remotes {
            let Some((_, p)) = info.iter().find(|(i, _)| *i == id) else {
                continue;
            };
            if !p.alive || Some(id) == me {
                continue;
            }
            let class = p
                .class
                .and_then(|c| session.view_mut().class_id(usize::from(c)))
                .unwrap_or_default();
            let primary = p
                .weapon
                .and_then(|w| session.view_mut().weapon_id(w))
                .unwrap_or_default();
            let selection = LoadoutSelection {
                class,
                primary,
                body_colour: body_colour(p.team, id.0),
                accent_colour: SLATE as u8,
                ..LoadoutSelection::default()
            };
            let feet = view.position - Vec3::Y * CENTRE_HEIGHT;
            let turn = Mat4::from_translation(feet) * Mat4::from_rotation_y(view.yaw);
            out.extend(self.cubes.character(&selection, turn));
        }

        // Pickups spin and bob where the server says they are available.
        let t = now as f32;
        for (i, pickup) in session.view_mut().pickups().into_iter().enumerate() {
            if !pickup.available {
                continue;
            }
            let bob = (t * 2.0 + i as f32).sin() * 0.08;
            let turn = Mat4::from_translation(pickup.position + Vec3::Y * bob)
                * Mat4::from_rotation_y(t * 1.5 + i as f32)
                * Mat4::from_rotation_x(0.6);
            out.push(self.cubes.part(
                pickup_colour(pickup.kind),
                Vec3::ZERO,
                Vec3::splat(0.35),
                turn,
            ));
        }

        // Projectiles in flight, blasts that just went off.
        for p in session.view_mut().projectiles(now).to_vec() {
            out.push(
                self.cubes
                    .part(METAL, p.position(now), Vec3::splat(0.22), Mat4::IDENTITY),
            );
        }
        for b in session.view_mut().blasts(now).to_vec() {
            let age = ((now - b.born) / crate::match_view::BLAST_SECONDS) as f32;
            out.push(self.cubes.part(
                AMBER,
                b.centre,
                Vec3::splat(0.5 + 2.5 * age.clamp(0.0, 1.0)),
                Mat4::from_rotation_y(age * 3.0),
            ));
        }

        out.extend(self.weapon(session, frame));
        out
    }

    /// The weapon in the local player's hands.
    fn weapon(&mut self, session: &mut Session, frame: &SceneFrame) -> Vec<Instance> {
        let view = session.view_mut();
        let Some(local) = view.local_player() else {
            return Vec::new();
        };
        for _ in 0..view.take_shots() {
            self.viewmodel.on_shot(&self.pattern);
        }
        // Stride phase: advances with the speed.
        self.phase = faset_assets::anim::advance_phase(self.phase, frame.speed, frame.dt);
        let dyaw = angle_difference(frame.look.yaw, self.prev_look.yaw);
        let dpitch = frame.look.pitch - self.prev_look.pitch;
        self.prev_look = frame.look;
        let offset = self.viewmodel.update(
            frame.dt,
            &ViewmodelInputs {
                look_delta: [dyaw, dpitch],
                speed: frame.speed,
                phase: self.phase,
                grounded: frame.grounded,
            },
        );
        if !local.alive {
            return Vec::new();
        }
        let id = local
            .weapon
            .and_then(|w| view.weapon_id(w))
            .unwrap_or_default();
        let size = weapon_size(&id) * VIEW_SCALE;
        let x = match frame.weapon_position {
            WeaponPosition::Right => 0.2,
            WeaponPosition::Left => -0.2,
            WeaponPosition::Center => 0.0,
        };
        let camera = Mat4::from_rotation_translation(frame.orientation, frame.eye);
        let held = camera * offset.to_mat4();
        let centre = Vec3::new(x, -0.19, -0.28 - size.z * 0.5);
        vec![
            self.cubes.part(METAL, centre, size, held),
            self.cubes.part(
                SKIN,
                Vec3::new(x, -0.22, -0.2),
                Vec3::new(0.07, 0.07, 0.12),
                held,
            ),
        ]
    }
}

/// Difference `a - b` of two angles in `(-π, π]`.
fn angle_difference(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI {
        d - std::f32::consts::TAU
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_difference_takes_the_short_way() {
        assert!((angle_difference(0.1, 6.2) - 0.183_185).abs() < 1e-4);
        assert!((angle_difference(6.2, 0.1) + 0.183_185).abs() < 1e-4);
        assert!(angle_difference(1.0, 1.0).abs() < 1e-6);
    }

    #[test]
    fn teams_and_free_for_all_get_distinct_colours() {
        assert_eq!(body_colour(Some(0), 3), AZURE as u8);
        assert_eq!(body_colour(Some(1), 3), CRIMSON as u8);
        assert_ne!(body_colour(None, 1), body_colour(None, 2));
    }

    #[test]
    fn every_pickup_kind_has_a_colour() {
        for kind in faset_mode_fps::KINDS {
            assert!(pickup_colour(kind) < faset_ui::PALETTE.len());
        }
    }
}
