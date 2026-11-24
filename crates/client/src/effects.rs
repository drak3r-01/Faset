//! Shot effects (lot 6.9): muzzle flash, tracers, material impacts and bullet
//! decals.
//!
//! Every pool is bounded: particles, tracers and decals live in fixed-capacity
//! ring buffers that drop their oldest entry when full ([`MAX_PARTICLES`],
//! [`MAX_TRACERS`], [`MAX_DECALS`]), so a long firefight never grows memory or
//! the instance list. The renderer has no particle system for gameplay, so the
//! effects are small boxes: emissive boxes (material [`EMISSIVE`], tinted by
//! the vertex colour) for flash, sparks and tracers, lit boxes for dust,
//! chips, splashes and decals. All of it is deterministic for a given seed.

use std::collections::VecDeque;

use faset_core::glam::{Mat4, Quat, Vec3};
use faset_render::{cube_mesh, Instance, MeshId, PointLight, RenderError, Renderer};

use crate::surface::Surface;

/// Particles alive at once.
pub const MAX_PARTICLES: usize = 192;
/// Tracers alive at once.
pub const MAX_TRACERS: usize = 24;
/// Bullet decals kept: a ring buffer, the oldest is replaced.
pub const MAX_DECALS: usize = 48;
/// Material id of the steady white lamp of the built-in table, whose emission
/// the vertex colour tints.
pub const EMISSIVE: u8 = 51;
/// Speed of a tracer streak, m/s.
pub const TRACER_SPEED: f32 = 320.0;
/// Length of a tracer streak, metres.
pub const TRACER_LENGTH: f32 = 4.0;
/// Seconds a muzzle flash lasts.
pub const FLASH_SECONDS: f32 = 0.055;
/// Seconds a bullet decal stays.
pub const DECAL_SECONDS: f32 = 20.0;
/// Seconds over which a decal shrinks away at the end of its life.
const DECAL_FADE: f32 = 3.0;

/// How a particle looks: one mesh each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    Spark,
    DustTan,
    DustGrey,
    Chip,
    Splash,
    Puff,
}

impl Look {
    const ALL: [Look; 6] = [
        Look::Spark,
        Look::DustTan,
        Look::DustGrey,
        Look::Chip,
        Look::Splash,
        Look::Puff,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// Vertex colour and material id of the mesh.
    fn paint(self) -> ([u8; 3], u8) {
        match self {
            Look::Spark => ([255, 190, 80], EMISSIVE),
            Look::DustTan => ([120, 92, 52], 0),
            Look::DustGrey => ([92, 92, 96], 0),
            Look::Chip => ([84, 50, 22], 0),
            Look::Splash => ([150, 205, 255], 0),
            Look::Puff => ([235, 240, 255], 0),
        }
    }
}

/// What one impact on a kind of surface throws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub look: Look,
    pub count: u32,
    /// Launch speed, m/s.
    pub speed: f32,
    /// Seconds alive.
    pub life: f32,
    /// Edge of the box, metres.
    pub size: f32,
    /// Downward acceleration, m/s^2.
    pub gravity: f32,
}

const fn burst(look: Look, count: u32, speed: f32, life: f32, size: f32, gravity: f32) -> Burst {
    Burst {
        look,
        count,
        speed,
        life,
        size,
        gravity,
    }
}

static METAL_BURSTS: [Burst; 2] = [
    burst(Look::Spark, 10, 6.0, 0.35, 0.03, 9.0),
    burst(Look::DustGrey, 2, 1.5, 0.4, 0.07, 0.0),
];
static CONCRETE_BURSTS: [Burst; 2] = [
    burst(Look::Spark, 4, 5.0, 0.3, 0.025, 9.0),
    burst(Look::DustGrey, 6, 2.2, 0.55, 0.09, 0.5),
];
static STONE_BURSTS: [Burst; 1] = [burst(Look::DustGrey, 8, 2.5, 0.6, 0.1, 0.5)];
static SAND_BURSTS: [Burst; 1] = [burst(Look::DustTan, 9, 2.0, 0.65, 0.1, 0.3)];
static WOOD_BURSTS: [Burst; 1] = [burst(Look::Chip, 7, 4.0, 0.5, 0.045, 9.0)];
static WATER_BURSTS: [Burst; 1] = [burst(Look::Splash, 10, 3.6, 0.55, 0.06, 9.0)];
static SNOW_BURSTS: [Burst; 1] = [burst(Look::Puff, 8, 1.8, 0.7, 0.1, 0.2)];

/// The particle bursts of an impact: sparks on metal and concrete, dust on
/// stone and sand, chips on wood, a splash on water, a puff on snow.
#[must_use]
pub fn impact_bursts(surface: Surface) -> &'static [Burst] {
    match surface {
        Surface::Metal => &METAL_BURSTS,
        Surface::Concrete => &CONCRETE_BURSTS,
        Surface::Stone => &STONE_BURSTS,
        Surface::Sand => &SAND_BURSTS,
        Surface::Wood => &WOOD_BURSTS,
        Surface::Water => &WATER_BURSTS,
        Surface::Snow => &SNOW_BURSTS,
    }
}

/// Whether a shot leaves a bullet decal on the surface (not on water).
#[must_use]
pub fn leaves_decal(surface: Surface) -> bool {
    surface != Surface::Water
}

#[derive(Debug, Clone, Copy)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: f32,
    gravity: f32,
    look: Look,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Tracer {
    from: Vec3,
    to: Vec3,
    age: f32,
}

/// The visible part `(tail, head)` of a tracer from `from` to `to` after
/// `age` seconds: a streak of [`TRACER_LENGTH`] flying at [`TRACER_SPEED`],
/// clipped to the segment. `None` once the streak has gone past `to`.
#[must_use]
pub fn tracer_segment(from: Vec3, to: Vec3, age: f32) -> Option<(Vec3, Vec3)> {
    let length = from.distance(to);
    if length < 1e-3 {
        return None;
    }
    let head = age * TRACER_SPEED;
    let tail = head - TRACER_LENGTH;
    if tail >= length {
        return None;
    }
    let dir = (to - from) / length;
    Some((
        from + dir * tail.clamp(0.0, length),
        from + dir * head.clamp(0.0, length),
    ))
}

#[derive(Debug, Clone, Copy)]
struct Decal {
    pos: Vec3,
    normal: Vec3,
    age: f32,
    roll: f32,
}

/// The effect pools.
#[derive(Debug, Clone)]
pub struct Effects {
    particles: VecDeque<Particle>,
    tracers: VecDeque<Tracer>,
    decals: VecDeque<Decal>,
    /// Age of the muzzle flash and its roll, while it lasts.
    flash: Option<(f32, f32)>,
    rng: u32,
}

impl Effects {
    /// Empty pools; `seed` drives every random choice.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            particles: VecDeque::with_capacity(MAX_PARTICLES),
            tracers: VecDeque::with_capacity(MAX_TRACERS),
            decals: VecDeque::with_capacity(MAX_DECALS),
            flash: None,
            rng: seed.max(1),
        }
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Two uniform values in `[-1, 1)`, for the spread of pellets.
    pub fn jitter(&mut self) -> (f32, f32) {
        (self.unit() * 2.0 - 1.0, self.unit() * 2.0 - 1.0)
    }

    fn push_particle(&mut self, p: Particle) {
        if self.particles.len() == MAX_PARTICLES {
            self.particles.pop_front();
        }
        self.particles.push_back(p);
    }

    /// An impact at `point` on a surface of kind `surface` facing `normal`:
    /// the burst of the surface and, unless it is water, a bullet decal.
    pub fn spawn_impact(&mut self, surface: Surface, point: Vec3, normal: Vec3) {
        let normal = normal.try_normalize().unwrap_or(Vec3::Y);
        for burst in impact_bursts(surface) {
            for _ in 0..burst.count {
                let jitter = Vec3::new(
                    self.unit() * 2.0 - 1.0,
                    self.unit() * 2.0 - 1.0,
                    self.unit() * 2.0 - 1.0,
                );
                let dir = (normal + jitter * 0.9).try_normalize().unwrap_or(normal);
                let speed = burst.speed * (0.5 + self.unit());
                let life = burst.life * (0.7 + 0.6 * self.unit());
                let size = burst.size * (0.7 + 0.6 * self.unit());
                self.push_particle(Particle {
                    pos: point + normal * 0.02,
                    vel: dir * speed,
                    age: 0.0,
                    life,
                    size,
                    gravity: burst.gravity,
                    look: burst.look,
                });
            }
        }
        if leaves_decal(surface) {
            if self.decals.len() == MAX_DECALS {
                self.decals.pop_front();
            }
            let roll = self.unit() * std::f32::consts::TAU;
            self.decals.push_back(Decal {
                pos: point,
                normal,
                age: 0.0,
                roll,
            });
        }
    }

    /// A tracer from the muzzle to the point the shot ended at.
    pub fn spawn_tracer(&mut self, from: Vec3, to: Vec3) {
        if self.tracers.len() == MAX_TRACERS {
            self.tracers.pop_front();
        }
        self.tracers.push_back(Tracer { from, to, age: 0.0 });
    }

    /// Starts a muzzle flash.
    pub fn trigger_flash(&mut self) {
        let roll = self.unit() * std::f32::consts::TAU;
        self.flash = Some((0.0, roll));
    }

    /// Strength of the muzzle flash, 1 when it starts and 0 when it is over.
    #[must_use]
    pub fn flash_strength(&self) -> f32 {
        self.flash
            .map_or(0.0, |(age, _)| (1.0 - age / FLASH_SECONDS).clamp(0.0, 1.0))
    }

    /// Ages everything by `dt` seconds and drops what has ended.
    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.age += dt;
            p.vel.y -= p.gravity * dt;
            p.vel *= (1.0 - 2.0 * dt).max(0.0);
            p.pos += p.vel * dt;
        }
        self.particles.retain(|p| p.age < p.life);
        for t in &mut self.tracers {
            t.age += dt;
        }
        self.tracers
            .retain(|t| tracer_segment(t.from, t.to, t.age).is_some());
        for d in &mut self.decals {
            d.age += dt;
        }
        self.decals.retain(|d| d.age < DECAL_SECONDS);
        if let Some((age, roll)) = self.flash {
            self.flash = (age + dt < FLASH_SECONDS).then_some((age + dt, roll));
        }
    }

    /// Particles alive.
    #[must_use]
    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }

    /// Tracers alive.
    #[must_use]
    pub fn tracer_count(&self) -> usize {
        self.tracers.len()
    }

    /// Decals kept.
    #[must_use]
    pub fn decal_count(&self) -> usize {
        self.decals.len()
    }

    /// The instances of the world effects: particles, tracers and decals.
    pub fn instances(&self, meshes: &EffectMeshes, out: &mut Vec<Instance>) {
        for p in &self.particles {
            let k = 1.0 - 0.7 * (p.age / p.life).clamp(0.0, 1.0);
            out.push(Instance::new(
                meshes.look(p.look),
                0,
                Mat4::from_translation(p.pos) * Mat4::from_scale(Vec3::splat(p.size * k)),
            ));
        }
        for t in &self.tracers {
            let Some((tail, head)) = tracer_segment(t.from, t.to, t.age) else {
                continue;
            };
            out.push(Instance::new(
                meshes.tracer,
                0,
                streak(tail, head, 0.018).unwrap_or(Mat4::ZERO),
            ));
        }
        for d in &self.decals {
            let fade = ((DECAL_SECONDS - d.age) / DECAL_FADE).clamp(0.0, 1.0);
            let turn = Quat::from_rotation_arc(Vec3::Z, d.normal) * Quat::from_rotation_z(d.roll);
            out.push(Instance::new(
                meshes.decal,
                0,
                Mat4::from_rotation_translation(turn, d.pos + d.normal * 0.012)
                    * Mat4::from_scale(Vec3::new(0.11 * fade, 0.11 * fade, 0.004)),
            ));
        }
    }

    /// The muzzle flash at `muzzle` (a world matrix whose -Z points along the
    /// barrel) for the view model, and its light. `None` outside the flash.
    #[must_use]
    pub fn flash(
        &self,
        meshes: &EffectMeshes,
        muzzle: Mat4,
    ) -> Option<(Vec<Instance>, PointLight)> {
        let (age, roll) = self.flash?;
        let s = self.flash_strength();
        let turn = muzzle * Mat4::from_rotation_z(roll + age * 40.0);
        let star = |w: f32, h: f32, extra: Mat4| {
            Instance::new(
                meshes.flash,
                0,
                turn * extra * Mat4::from_scale(Vec3::new(w * s, h * s, 0.02)),
            )
        };
        let forward = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.05 * s));
        let instances = vec![
            star(0.26, 0.05, forward),
            star(0.05, 0.26, forward),
            star(
                0.17,
                0.05,
                forward * Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4),
            ),
            star(
                0.17,
                0.05,
                forward * Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_4),
            ),
            Instance::new(
                meshes.flash,
                0,
                turn * forward * Mat4::from_scale(Vec3::splat(0.09 * s)),
            ),
        ];
        let position = muzzle.transform_point3(Vec3::new(0.0, 0.0, -0.25));
        Some((
            instances,
            PointLight {
                position,
                color: Vec3::new(3.0, 2.0, 0.9) * s,
                radius: 7.0,
            },
        ))
    }
}

/// The matrix of a unit box stretched along `tail -> head` with a square
/// section of `width`; `None` for a degenerate segment.
fn streak(tail: Vec3, head: Vec3, width: f32) -> Option<Mat4> {
    let length = tail.distance(head);
    if length < 1e-4 {
        return None;
    }
    let dir = (head - tail) / length;
    Some(
        Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Z, dir), (tail + head) * 0.5)
            * Mat4::from_scale(Vec3::new(width, width, length)),
    )
}

/// The meshes of the effects, uploaded once per renderer.
#[derive(Debug, Clone)]
pub struct EffectMeshes {
    looks: Vec<MeshId>,
    pub tracer: MeshId,
    pub flash: MeshId,
    pub decal: MeshId,
}

impl EffectMeshes {
    /// Uploads the boxes.
    ///
    /// # Errors
    /// The renderer refused a mesh (a full arena).
    pub fn upload(renderer: &mut Renderer) -> Result<Self, RenderError> {
        let mut cube = |rgb: [u8; 3], material: u8| {
            let (v, i) = cube_mesh(0.5, &[rgb; 6], material);
            renderer.upload_mesh(&v, &i)
        };
        let mut looks = Vec::with_capacity(Look::ALL.len());
        for look in Look::ALL {
            let (rgb, material) = look.paint();
            looks.push(cube(rgb, material)?);
        }
        Ok(Self {
            looks,
            tracer: cube([255, 236, 170], EMISSIVE)?,
            flash: cube([255, 214, 120], EMISSIVE)?,
            decal: cube([10, 10, 12], 0)?,
        })
    }

    fn look(&self, look: Look) -> MeshId {
        self.looks[look.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_has_a_burst_and_water_has_no_decal() {
        for s in Surface::ALL {
            assert!(!impact_bursts(s).is_empty(), "{s:?}");
            assert_eq!(leaves_decal(s), s != Surface::Water);
        }
        // Sparks on metal and concrete, chips on wood, a splash on water.
        let has = |s, look| impact_bursts(s).iter().any(|b| b.look == look);
        assert!(has(Surface::Metal, Look::Spark));
        assert!(has(Surface::Concrete, Look::Spark));
        assert!(has(Surface::Stone, Look::DustGrey));
        assert!(has(Surface::Sand, Look::DustTan));
        assert!(has(Surface::Wood, Look::Chip));
        assert!(has(Surface::Water, Look::Splash));
        assert!(has(Surface::Snow, Look::Puff));
    }

    #[test]
    fn pools_are_bounded_however_much_is_fired() {
        let mut fx = Effects::new(7);
        for i in 0..5_000 {
            let at = Vec3::new(i as f32, 0.0, 0.0);
            fx.spawn_impact(Surface::ALL[i % Surface::ALL.len()], at, Vec3::Y);
            fx.spawn_tracer(Vec3::ZERO, at + Vec3::X * 50.0);
            fx.trigger_flash();
            assert!(fx.particle_count() <= MAX_PARTICLES);
            assert!(fx.tracer_count() <= MAX_TRACERS);
            assert!(fx.decal_count() <= MAX_DECALS);
        }
        assert_eq!(fx.particle_count(), MAX_PARTICLES);
        assert_eq!(fx.tracer_count(), MAX_TRACERS);
        assert_eq!(fx.decal_count(), MAX_DECALS);
    }

    #[test]
    fn the_decal_ring_replaces_the_oldest() {
        let mut fx = Effects::new(1);
        for i in 0..MAX_DECALS + 3 {
            fx.spawn_impact(Surface::Concrete, Vec3::new(i as f32, 0.0, 0.0), Vec3::Y);
        }
        assert_eq!(fx.decal_count(), MAX_DECALS);
        assert_eq!(fx.decals.front().unwrap().pos.x, 3.0, "oldest three gone");
        assert_eq!(
            fx.decals.back().unwrap().pos.x,
            (MAX_DECALS + 2) as f32,
            "newest kept"
        );
    }

    #[test]
    fn everything_ends_by_itself() {
        let mut fx = Effects::new(3);
        fx.spawn_impact(Surface::Metal, Vec3::ZERO, Vec3::Y);
        fx.spawn_impact(Surface::Water, Vec3::ZERO, Vec3::Y);
        fx.spawn_tracer(Vec3::ZERO, Vec3::new(0.0, 0.0, -100.0));
        fx.trigger_flash();
        assert!(fx.flash_strength() > 0.99);
        for _ in 0..100 {
            fx.update(0.01);
        }
        assert_eq!(fx.particle_count(), 0, "particles live under 1.2 s");
        assert_eq!(fx.tracer_count(), 0);
        assert_eq!(fx.flash_strength(), 0.0);
        assert_eq!(fx.decal_count(), 1, "the decal stays");
        for _ in 0..(DECAL_SECONDS * 100.0) as usize {
            fx.update(0.01);
        }
        assert_eq!(fx.decal_count(), 0);
    }

    #[test]
    fn the_same_seed_throws_the_same_particles() {
        let run = |seed| {
            let mut fx = Effects::new(seed);
            fx.spawn_impact(Surface::Sand, Vec3::ZERO, Vec3::Y);
            fx.update(0.1);
            fx.particles.iter().map(|p| p.pos).collect::<Vec<_>>()
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    #[test]
    fn particles_leave_the_surface_they_hit() {
        let mut fx = Effects::new(11);
        let normal = Vec3::new(0.0, 0.0, 1.0);
        fx.spawn_impact(Surface::Stone, Vec3::ZERO, normal);
        let out = fx
            .particles
            .iter()
            .filter(|p| p.vel.dot(normal) > 0.0)
            .count();
        assert_eq!(out, fx.particle_count(), "all start away from the wall");
    }

    #[test]
    fn a_tracer_is_a_streak_that_flies_to_the_hit_and_ends() {
        let from = Vec3::ZERO;
        let to = Vec3::new(0.0, 0.0, -64.0);
        // Starts as a point at the muzzle: head and tail are both there.
        let (tail, head) = tracer_segment(from, to, 0.0).unwrap();
        assert_eq!((tail, head), (from, from));
        // Flying: head ahead of the tail by the streak length.
        let (tail, head) = tracer_segment(from, to, 0.1).unwrap();
        assert!((head.z + 32.0).abs() < 1e-3);
        assert!((head.distance(tail) - TRACER_LENGTH).abs() < 1e-3);
        // Never past the hit point.
        let (_, head) = tracer_segment(from, to, 0.19).unwrap();
        assert!((head.z + 60.8).abs() < 1e-2);
        let (_, head) = tracer_segment(from, to, 0.205).unwrap();
        assert_eq!(head, to);
        // Over once the tail reaches it.
        assert!(tracer_segment(from, to, (64.0 + TRACER_LENGTH) / TRACER_SPEED + 0.01).is_none());
        assert!(tracer_segment(from, from, 0.1).is_none());
    }

    #[test]
    fn a_streak_lies_along_its_segment() {
        let m = streak(Vec3::ZERO, Vec3::new(0.0, 0.0, -2.0), 0.1).unwrap();
        assert!((m.transform_point3(Vec3::ZERO) - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
        assert!(
            (m.transform_point3(Vec3::new(0.0, 0.0, 0.5)) - Vec3::new(0.0, 0.0, -2.0)).length()
                < 1e-5
        );
        assert!(streak(Vec3::ZERO, Vec3::ZERO, 0.1).is_none());
    }

    #[test]
    fn the_flash_fades_with_its_light() {
        let meshes = EffectMeshes {
            looks: vec![MeshId(0); Look::ALL.len()],
            tracer: MeshId(0),
            flash: MeshId(0),
            decal: MeshId(0),
        };
        let mut fx = Effects::new(2);
        assert!(fx.flash(&meshes, Mat4::IDENTITY).is_none());
        fx.trigger_flash();
        let (instances, light) = fx.flash(&meshes, Mat4::IDENTITY).unwrap();
        assert_eq!(instances.len(), 5);
        let first = light.color.x;
        fx.update(FLASH_SECONDS * 0.5);
        let (_, light) = fx.flash(&meshes, Mat4::IDENTITY).unwrap();
        assert!(light.color.x < first && light.color.x > 0.0);
        fx.update(FLASH_SECONDS);
        assert!(fx.flash(&meshes, Mat4::IDENTITY).is_none());
    }
}
