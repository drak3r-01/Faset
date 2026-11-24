//! What a match shows besides the map (lot 6.6): the other players, the
//! pickups, the projectiles and blasts, and the weapon in the local
//! player's hands.
//!
//! The other players are built from unit boxes through [`CharacterPreview`]'s
//! cubes, the same way the loadout screen draws its character, carrying the
//! detailed weapon models of [`crate::weapon_models`]; no new renderer
//! interface is needed. The weapon in hand is animated by the springs of
//! `faset-assets` (sway, step, recoil).
//!
//! Lot 6.9 adds the feel of shooting. The weapon is handed to the renderer as
//! a separate view model (its own depth range, so a wall never swallows it),
//! every shot of the local player spawns a muzzle flash, a tracer and a
//! material impact, and the frame reports the sounds to play, the camera shake
//! and the directions of the damage taken.
//!
//! Decision: the tracer and the impact follow the ray from the eye along the
//! view direction against the map's static collision (the same rays the
//! server uses for walls); players are not tested, so a shot that hits a
//! player still draws its tracer to the wall behind. Only the local player's
//! shots are drawn: the match does not tell clients where other players shot.

use std::sync::Arc;

use faset_assets::viewmodel::{RecoilPattern, Viewmodel, ViewmodelInputs};
use faset_core::glam::Vec3;
use faset_core::settings::WeaponPosition;
use faset_core::NetId;
use faset_mode_fps::{PickupKind, WeaponDef, WeaponKind};
use faset_render::glam::{Mat4, Quat};
use faset_render::{Instance, PointLight, RenderError, Renderer};
use faset_sim::StaticWorld;
use faset_ui::LoadoutSelection;

use crate::camera::Look;
use crate::effects::{EffectMeshes, Effects};
use crate::match_view::Feedback;
use crate::preview::{weapon_size, CharacterPreview, METAL, SKIN};
use crate::session::Session;
use crate::shoot_feel::{
    blast_trauma, recoil_pattern, shot_trauma, CameraShake, DamageArc, DamageIndicators,
};
use crate::sounds::{shot_recipe, Cue};
use crate::surface::{Surface, SurfaceMap};
use crate::weapon_models::view_grip;

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
const VIEW_SCALE: f32 = 0.55;
/// Longest shot the client draws a tracer and an impact for, metres.
const TRACE_RANGE: f32 = 120.0;
/// Eye height above the feet, to find the surface under a footstep.
const EYE_ABOVE_FEET: f32 = 1.6;
/// Most pellets of one shot that get a tracer and an impact.
const MAX_TRACED_PELLETS: u32 = 4;

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

/// What the scene hands to the frame (lot 6.9).
#[derive(Debug, Default)]
pub struct SceneOutput {
    /// Other players, pickups, projectiles, blasts and the shot effects: the
    /// instances of the world.
    pub world: Vec<Instance>,
    /// The weapon in hand and its muzzle flash, for
    /// [`RenderWorld::viewmodel`](faset_render::RenderWorld::viewmodel).
    pub viewmodel: Vec<Instance>,
    /// The light of the muzzle flash.
    pub lights: Vec<PointLight>,
    /// The sounds of the frame; no position means the player's own.
    pub cues: Vec<(Cue, Option<Vec3>)>,
    /// Camera shake to apply to the view (skip it with reduced motion).
    pub shake: Quat,
    /// Where the damage the player took came from, for the HUD.
    pub damage: Vec<DamageArc>,
}

/// The weapon in hand for one frame.
#[derive(Default)]
struct Held {
    instances: Vec<Instance>,
    /// World matrix of the muzzle, -Z along the barrel.
    muzzle: Option<Mat4>,
}

/// The meshes and animation state of a match's scene.
pub struct MatchScene {
    cubes: CharacterPreview,
    fx_meshes: EffectMeshes,
    effects: Effects,
    viewmodel: Viewmodel,
    pattern: RecoilPattern,
    /// Weapon the pattern was built for.
    pattern_for: Option<usize>,
    prev_look: Look,
    phase: f32,
    /// The stride phase crossed a half turn this frame: a footstep.
    stepped: bool,
    surfaces: Arc<SurfaceMap>,
    shake: CameraShake,
    damage: DamageIndicators,
    cues: Vec<(Cue, Option<Vec3>)>,
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
            fx_meshes: EffectMeshes::upload(renderer)?,
            effects: Effects::new(0x5eed_f00d),
            viewmodel: Viewmodel::new(1),
            pattern: RecoilPattern::test_rifle(),
            pattern_for: None,
            prev_look: Look::default(),
            phase: 0.0,
            stepped: false,
            surfaces: Arc::new(SurfaceMap::default()),
            shake: CameraShake::default(),
            damage: DamageIndicators::default(),
            cues: Vec::new(),
        })
    }

    /// The surfaces of the map being played, for impacts and footsteps.
    pub fn set_surfaces(&mut self, surfaces: Arc<SurfaceMap>) {
        self.surfaces = surfaces;
    }

    /// The scene at the session's present: other players, pickups,
    /// projectiles, blasts, the shot effects, the local weapon and what the
    /// frame must play, shake and show.
    pub fn instances(&mut self, session: &mut Session, frame: &SceneFrame) -> SceneOutput {
        let now = session.time();
        let mut out = Vec::new();
        self.effects.update(frame.dt);
        self.shake.update(frame.dt);
        self.damage.update(frame.dt);

        // The other players, interpolated in the past.
        let render = session
            .remotes()
            .render_time(now, session.clock().jitter())
            .unwrap_or(0.0);
        let remotes: Vec<_> = session.remotes().sample_all(render).collect();
        let positions: Vec<(NetId, Vec3)> =
            remotes.iter().map(|(id, v)| (*id, v.position)).collect();
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

        self.feedback(session, frame, &positions);
        let held = self.weapon(session, frame);
        self.footsteps(session.statics(), frame);

        self.effects.instances(&self.fx_meshes, &mut out);
        let mut viewmodel = held.instances;
        let mut lights = Vec::new();
        if let Some((flash, light)) = held
            .muzzle
            .and_then(|muzzle| self.effects.flash(&self.fx_meshes, muzzle))
        {
            viewmodel.extend(flash);
            lights.push(light);
        }
        SceneOutput {
            world: out,
            viewmodel,
            lights,
            cues: std::mem::take(&mut self.cues),
            shake: self.shake.rotation(),
            damage: self.damage.arcs(frame.eye, frame.orientation * Vec3::NEG_Z),
        }
    }

    /// Turns what the match reported into sounds, shake and damage arcs.
    fn feedback(&mut self, session: &mut Session, frame: &SceneFrame, positions: &[(NetId, Vec3)]) {
        for event in session.view_mut().take_feedback() {
            match event {
                Feedback::Reloaded => self.cues.push((Cue::Reload, None)),
                Feedback::HitConfirmed { .. } => self.cues.push((Cue::Hit, None)),
                Feedback::KillConfirmed => self.cues.push((Cue::Kill, None)),
                Feedback::Damaged { attacker } => {
                    if let Some((_, from)) = positions.iter().find(|(id, _)| *id == attacker) {
                        self.damage.push(*from);
                    }
                    self.cues.push((Cue::Hurt, None));
                    self.shake.add(0.3);
                }
                Feedback::Explosion { centre } => {
                    self.cues.push((Cue::Explosion, Some(centre)));
                    self.shake.add(blast_trauma(centre.distance(frame.eye)));
                }
            }
        }
    }

    /// A footstep each time the stride phase passes a half turn, on the
    /// surface under the feet.
    fn footsteps(&mut self, statics: &StaticWorld, frame: &SceneFrame) {
        if !self.stepped || !frame.grounded || frame.speed < 1.5 {
            return;
        }
        let feet = frame.eye.y - EYE_ABOVE_FEET;
        let surface = if self
            .surfaces
            .water_height()
            .is_some_and(|level| feet < level)
        {
            Surface::Water
        } else {
            statics
                .ray_cast(frame.eye, Vec3::NEG_Y, EYE_ABOVE_FEET + 1.0)
                .map_or_else(Surface::default, |hit| self.surfaces.of_volume(hit.volume))
        };
        self.cues.push((Cue::Footstep(surface), None));
    }

    /// The weapon in the local player's hands, and the effects of the shots
    /// the player fired since the last frame.
    fn weapon(&mut self, session: &mut Session, frame: &SceneFrame) -> Held {
        let view = session.view_mut();
        let Some(local) = view.local_player() else {
            return Held::default();
        };
        let shots = view.take_shots();
        let def = local.weapon.and_then(|w| view.weapon_def(w));
        let id = local
            .weapon
            .and_then(|w| view.weapon_id(w))
            .unwrap_or_default();
        if self.pattern_for != local.weapon {
            if let Some(def) = &def {
                self.pattern = recoil_pattern(def);
            }
            self.pattern_for = local.weapon;
        }
        if local.alive {
            for _ in 0..shots {
                self.viewmodel.on_shot(&self.pattern);
            }
        }
        // Stride phase: advances with the speed.
        let before = self.phase;
        self.phase = faset_assets::anim::advance_phase(self.phase, frame.speed, frame.dt);
        let half_turn = std::f32::consts::PI;
        self.stepped = (before / half_turn).floor() != (self.phase / half_turn).floor();
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
            return Held::default();
        }
        let x = match frame.weapon_position {
            WeaponPosition::Right => 0.2,
            WeaponPosition::Left => -0.2,
            WeaponPosition::Center => 0.0,
        };
        let camera = Mat4::from_rotation_translation(frame.orientation, frame.eye);
        let held = camera * offset.to_mat4();
        let mut out = Held::default();
        if let Some(model) = self.cubes.models().get(&id) {
            let grip = view_grip(x, model.length);
            let world =
                held * Mat4::from_translation(grip) * Mat4::from_scale(Vec3::splat(VIEW_SCALE));
            out.instances.push(Instance::new(model.mesh, 0, world));
            out.muzzle = Some(
                held * Mat4::from_translation(grip)
                    * Mat4::from_translation(Vec3::new(0.0, 0.0, -model.length * VIEW_SCALE)),
            );
            // Right hand on the grip, left hand on the fore-end, sleeves back
            // towards the body.
            let reach = -0.3 * model.length * VIEW_SCALE - 0.06;
            for (hand, sleeve_dx) in [
                (Vec3::new(0.0, -0.025, 0.03), 0.04),
                (Vec3::new(-0.02, 0.0, reach), -0.06),
            ] {
                out.instances.push(self.cubes.part(
                    SKIN,
                    grip + hand,
                    Vec3::new(0.065, 0.07, 0.1),
                    held,
                ));
                out.instances.push(self.cubes.part(
                    SLATE,
                    grip + hand + Vec3::new(sleeve_dx, -0.04, 0.15),
                    Vec3::new(0.07, 0.07, 0.2),
                    held,
                ));
            }
        } else {
            let size = weapon_size(&id) * VIEW_SCALE;
            let centre = Vec3::new(x, -0.19, -0.28 - size.z * 0.5);
            out.instances
                .push(self.cubes.part(METAL, centre, size, held));
            out.instances.push(self.cubes.part(
                SKIN,
                Vec3::new(x, -0.22, -0.2),
                Vec3::new(0.07, 0.07, 0.12),
                held,
            ));
            out.muzzle = Some(held * Mat4::from_translation(centre - Vec3::Z * size.z * 0.5));
        }
        if let (Some(def), Some(muzzle)) = (&def, out.muzzle) {
            for _ in 0..shots {
                self.fire(session.statics(), frame, def, &id, muzzle);
            }
        }
        out
    }

    /// The effects of one shot of the local player: flash, sound, shake and,
    /// for a hitscan weapon, the tracers and impacts.
    fn fire(
        &mut self,
        statics: &StaticWorld,
        frame: &SceneFrame,
        def: &WeaponDef,
        id: &str,
        muzzle: Mat4,
    ) {
        self.cues.push((Cue::Shot(shot_recipe(id)), None));
        if def.kind == WeaponKind::Melee {
            return;
        }
        self.effects.trigger_flash();
        self.shake.add(shot_trauma(def));
        if def.kind != WeaponKind::Hitscan {
            return;
        }
        let forward = frame.orientation * Vec3::NEG_Z;
        let from = muzzle.transform_point3(Vec3::ZERO);
        let right = forward.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = right.cross(forward);
        let range = def.range.min(TRACE_RANGE);
        for pellet in 0..def.pellets.clamp(1, MAX_TRACED_PELLETS) {
            let dir = if pellet == 0 {
                forward
            } else {
                // The other pellets spread over the weapon's cone.
                let (jx, jy) = self.effects.jitter();
                let s = def.spread.sigma_base.max(0.01) * 2.0;
                (forward + right * jx * s + up * jy * s).normalize_or(forward)
            };
            self.trace(statics, frame.eye, dir, range, from);
        }
    }

    /// One bullet from the eye along `dir`: its tracer from the muzzle and,
    /// where it meets the map or the sea, an impact and its sound.
    fn trace(&mut self, statics: &StaticWorld, eye: Vec3, dir: Vec3, range: f32, muzzle: Vec3) {
        let hit = statics.ray_cast(eye, dir, range);
        let water = self
            .surfaces
            .water_hit(eye, dir, hit.map_or(range, |h| h.distance));
        let (end, impact) = match (water, hit) {
            (Some(t), _) => {
                let point = eye + dir * t;
                (point, Some((Surface::Water, point, Vec3::Y)))
            }
            (None, Some(h)) => {
                let point = eye + dir * h.distance;
                (
                    point,
                    Some((self.surfaces.of_volume(h.volume), point, h.normal)),
                )
            }
            (None, None) => (eye + dir * range, None),
        };
        self.effects.spawn_tracer(muzzle, end);
        if let Some((surface, point, normal)) = impact {
            self.effects.spawn_impact(surface, point, normal);
            self.cues.push((Cue::Impact(surface), Some(point)));
        }
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
