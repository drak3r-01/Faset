//! Top and perspective views of a map, rendered offscreen with `render`.
//!
//! The top view looks straight down with -Z at the top of the image and has
//! spawns, pickups and zones drawn over it.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use faset_client::LoadedMap;
use faset_mapfmt::{grid_to_metres, PickupKind, Team, Zone, ZoneKind};
use faset_render::glam::{Mat4, Vec3, Vec4};
use faset_render::{jitter, perspective_reverse_z, Camera, RenderConfig, RenderWorld, Renderer};
use image::{Rgba, RgbaImage};

/// Default image size of the views.
pub const VIEW_SIZE: (u32, u32) = (640, 480);

const FOV_DEG: f32 = 40.0;
const NEAR_M: f32 = 0.1;
const CLEAR: [f32; 4] = [0.25, 0.35, 0.5, 1.0];

/// Which view to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Straight down, with spawns, pickups and zones marked.
    Top,
    /// Three-quarter view of the whole map.
    Perspective,
}

/// A headless renderer holding one loaded map.
pub struct Viewer {
    renderer: Renderer,
    instances: Vec<faset_render::Instance>,
    bounds: (Vec3, Vec3),
    size: (u32, u32),
    counter: u32,
}

/// A rendered image and the matrices that produced it.
struct Shot {
    image: RgbaImage,
    view: Mat4,
    proj: Mat4,
}

impl Viewer {
    /// Uploads `map` to a new headless renderer of `size` pixels.
    ///
    /// # Errors
    /// No Vulkan device, an empty map, or a mesh upload failure.
    pub fn new(map: &LoadedMap, size: (u32, u32)) -> Result<Self> {
        let mut renderer = Renderer::new_headless(&RenderConfig {
            size,
            ..RenderConfig::default()
        })
        .context("cannot create a headless renderer (is a Vulkan driver installed?)")?;
        let instances = map.upload(&mut renderer)?;
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for v in map.chunks.iter().flat_map(|c| &c.vertices) {
            let p = Vec3::from_array(v.position);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        if instances.is_empty() || lo.x > hi.x {
            bail!("the map has no geometry to draw");
        }
        Ok(Self {
            renderer,
            instances,
            bounds: (lo, hi),
            size,
            counter: 0,
        })
    }

    /// Vulkan validation messages seen so far.
    #[must_use]
    pub fn validation_message_count(&self) -> u32 {
        self.renderer.validation_message_count()
    }

    #[allow(clippy::cast_precision_loss)]
    fn camera(&self, view: View) -> (Mat4, Mat4) {
        let (lo, hi) = self.bounds;
        let centre = (lo + hi) * 0.5;
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        let half_tan = (0.5 * FOV_DEG.to_radians()).tan();
        let (eye, up) = match view {
            View::Top => {
                let fit = ((hi.z - lo.z) * 0.5).max((hi.x - lo.x) * 0.5 / aspect);
                let height = fit / half_tan * 1.08 + hi.y;
                (Vec3::new(centre.x, height, centre.z), Vec3::NEG_Z)
            }
            View::Perspective => {
                let radius = (hi - lo).length() * 0.5;
                let dir = Vec3::new(-0.25, 0.7, 0.65).normalize();
                (centre + dir * radius / half_tan * 0.85, Vec3::Y)
            }
        };
        let target = if view == View::Top {
            Vec3::new(centre.x, 0.0, centre.z)
        } else {
            centre
        };
        let look = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, up);
        (
            look,
            perspective_reverse_z(FOV_DEG.to_radians(), aspect, NEAR_M),
        )
    }

    fn shoot(&mut self, view: View) -> Result<Shot> {
        let (v, p) = self.camera(view);
        for frame in 0..4u32 {
            let mut camera = Camera::new(v, p);
            camera.jitter_px = jitter(frame);
            self.renderer.render_frame(&RenderWorld {
                clear_color: CLEAR,
                camera,
                instances: self.instances.clone(),
                ..RenderWorld::default()
            })?;
        }
        self.counter += 1;
        let path: PathBuf = std::env::temp_dir().join(format!(
            "map-tool-{}-{}.png",
            std::process::id(),
            self.counter
        ));
        self.renderer.read_back_png(&path)?;
        let image = image::open(&path)?.to_rgba8();
        let _ = std::fs::remove_file(&path);
        Ok(Shot {
            image,
            view: v,
            proj: p,
        })
    }

    /// The perspective view.
    ///
    /// # Errors
    /// Render or read-back failure.
    pub fn perspective(&mut self) -> Result<RgbaImage> {
        Ok(self.shoot(View::Perspective)?.image)
    }

    /// The top view with spawns, pickups and zones drawn over it.
    ///
    /// # Errors
    /// Render or read-back failure.
    pub fn top(&mut self, map: &LoadedMap) -> Result<RgbaImage> {
        let mut shot = self.shoot(View::Top)?;
        overlay(&mut shot, map);
        Ok(shot.image)
    }
}

fn project(shot: &Shot, p: Vec3) -> Option<(f32, f32)> {
    let clip = shot.proj * shot.view * Vec4::new(p.x, p.y, p.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (w, h) = shot.image.dimensions();
    #[allow(clippy::cast_precision_loss)]
    Some((
        (clip.x / clip.w * 0.5 + 0.5) * w as f32,
        (clip.y / clip.w * 0.5 + 0.5) * h as f32,
    ))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn put(img: &mut RgbaImage, x: i64, y: i64, c: Rgba<u8>) {
    let (w, h) = img.dimensions();
    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
        img.put_pixel(x as u32, y as u32, c);
    }
}

#[allow(clippy::cast_possible_truncation)]
fn disc(img: &mut RgbaImage, at: (f32, f32), r: i64, fill: Rgba<u8>) {
    let (cx, cy) = (at.0.round() as i64, at.1.round() as i64);
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 <= r * r {
                let edge = d2 > (r - 1) * (r - 1);
                put(
                    img,
                    cx + dx,
                    cy + dy,
                    if edge { Rgba([0, 0, 0, 255]) } else { fill },
                );
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn square(img: &mut RgbaImage, at: (f32, f32), r: i64, fill: Rgba<u8>) {
    let (cx, cy) = (at.0.round() as i64, at.1.round() as i64);
    for dy in -r..=r {
        for dx in -r..=r {
            let edge = dx.abs() == r || dy.abs() == r;
            put(
                img,
                cx + dx,
                cy + dy,
                if edge { Rgba([0, 0, 0, 255]) } else { fill },
            );
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn line(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), c: Rgba<u8>) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0) as i64;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        put(
            img,
            (a.0 + (b.0 - a.0) * t).round() as i64,
            (a.1 + (b.1 - a.1) * t).round() as i64,
            c,
        );
    }
}

fn overlay(shot: &mut Shot, map: &LoadedMap) {
    // Zones first, so markers stay readable. Kill volumes cover the whole
    // map and are left out.
    for Zone::Box { min, max, kind } in &map.map.zones {
        let colour = match kind {
            ZoneKind::KillVolume => continue,
            ZoneKind::Water => Rgba([60, 120, 255, 255]),
            ZoneKind::JumpPad { .. } => Rgba([80, 255, 120, 255]),
            ZoneKind::Teleporter { .. } => Rgba([220, 90, 255, 255]),
            ZoneKind::LowGravity { .. } => Rgba([200, 200, 255, 255]),
            ZoneKind::Objective { .. } => Rgba([255, 160, 40, 255]),
        };
        let (lo, hi) = (grid_to_metres(*min), grid_to_metres(*max));
        let corners = [
            Vec3::new(lo.x, hi.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(hi.x, hi.y, hi.z),
            Vec3::new(lo.x, hi.y, hi.z),
        ]
        .map(|c| project(shot, c));
        for i in 0..4 {
            if let (Some(a), Some(b)) = (corners[i], corners[(i + 1) % 4]) {
                line(&mut shot.image, a, b, colour);
            }
        }
    }
    for p in &map.map.pickups {
        let colour = match p.kind {
            PickupKind::Health => Rgba([60, 220, 80, 255]),
            PickupKind::Armor => Rgba([80, 160, 255, 255]),
            PickupKind::Ammo => Rgba([240, 220, 60, 255]),
            PickupKind::Weapon(_) => Rgba([255, 120, 40, 255]),
        };
        if let Some(at) = project(shot, Vec3::from_array(p.at)) {
            square(&mut shot.image, at, 4, colour);
        }
    }
    for s in &map.map.spawns {
        let colour = match s.team {
            Team::A => Rgba([235, 60, 60, 255]),
            Team::B => Rgba([70, 110, 245, 255]),
            Team::Any => Rgba([240, 240, 240, 255]),
        };
        if let Some(at) = project(shot, Vec3::from_array(s.at)) {
            disc(&mut shot.image, at, 5, colour);
        }
    }
}
