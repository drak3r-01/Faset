//! Library half of `asset-tool`: recipe discovery, validation, offscreen
//! thumbnails and the contact sheet.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use faset_assets::{build_mesh, validate, Mesh, Palette, Recipe, Validation};
use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    jitter, perspective_reverse_z, AmbienceProfile, Camera, Instance, MaterialTable, RenderConfig,
    RenderWorld, Renderer, Vertex,
};

/// Thumbnail edge in pixels.
pub const THUMB_SIZE: u32 = 256;

/// One recipe file and what was found in it.
pub struct Entry {
    pub path: PathBuf,
    pub recipe: Option<Recipe>,
    pub validation: Validation,
}

/// Lists `*.ron` files of `path` (a file or a directory), sorted by name.
/// A `palette.ron` file is not a recipe.
pub fn recipe_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_owned()]);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "ron")
                && p.file_name().is_some_and(|n| n != "palette.ron")
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Palette of a directory: its `palette.ron` when present, else the default.
pub fn load_palette(path: &Path) -> Result<Palette> {
    let file = if path.is_dir() {
        path.join("palette.ron")
    } else {
        path.with_file_name("palette.ron")
    };
    if file.is_file() {
        let text = std::fs::read_to_string(&file)?;
        return Palette::from_ron(&text).with_context(|| format!("bad palette {}", file.display()));
    }
    Ok(Palette::default())
}

/// Parses and validates every recipe under `path`.
pub fn validate_path(path: &Path, palette: &Palette) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for file in recipe_files(path)? {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        let entry = match Recipe::from_ron(&text) {
            Ok(recipe) => {
                let validation = validate(&recipe, palette);
                Entry {
                    path: file,
                    recipe: Some(recipe),
                    validation,
                }
            }
            Err(e) => Entry {
                validation: Validation {
                    id: file
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    kind: faset_assets::AssetKind::Prop,
                    triangles: 0,
                    vertices: 0,
                    size: [0.0; 3],
                    errors: vec![format!("syntax: {e}")],
                },
                path: file,
                recipe: None,
            },
        };
        entries.push(entry);
    }
    Ok(entries)
}

/// Renders meshes offscreen (lavapipe in Docker) without a window.
pub struct Thumbnailer {
    renderer: Renderer,
    size: u32,
    counter: u32,
}

impl Thumbnailer {
    pub fn new(size: u32) -> Result<Self> {
        let mut renderer = Renderer::new_headless(&RenderConfig {
            size: (size, size),
            ..RenderConfig::default()
        })
        .context("cannot create a headless renderer (is a Vulkan driver installed?)")?;
        renderer.set_materials(&MaterialTable::builtin())?;
        Ok(Self {
            renderer,
            size,
            counter: 0,
        })
    }

    /// Vulkan validation messages seen so far.
    pub fn validation_message_count(&self) -> u64 {
        u64::from(self.renderer.validation_message_count())
    }

    /// Renders `mesh` from a three-quarter view that frames its bounds.
    pub fn render(&mut self, mesh: &Mesh) -> Result<image::RgbaImage> {
        let Some((lo, hi)) = mesh.bounds() else {
            bail!("empty mesh");
        };
        let vertices: Vec<Vertex> = mesh
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                color_material: v.color_material,
            })
            .collect();
        let id = self.renderer.upload_mesh(&vertices, &mesh.indices)?;

        let (centre, radius) = (
            Vec3::from_array((lo + hi).to_array()) * 0.5,
            (hi - lo).length() * 0.5,
        );
        let fov = 35f32.to_radians();
        let distance = radius / (0.5 * fov).sin() * 1.1;
        let eye = centre + Vec3::new(0.8, 0.6, 1.0).normalize() * distance;
        let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, centre, Vec3::Y);
        let proj = perspective_reverse_z(fov, 1.0, 0.05);
        let world = |camera| RenderWorld {
            clear_color: [0.22, 0.25, 0.3, 1.0],
            camera,
            instances: vec![Instance::new(id, 0, Mat4::IDENTITY)],
            ambience: AmbienceProfile::builtin("day").expect("built-in profile"),
            ..RenderWorld::default()
        };
        for frame in 0..4u32 {
            let mut camera = Camera::new(view, proj);
            camera.jitter_px = jitter(frame);
            self.renderer.render_frame(&world(camera))?;
        }
        self.counter += 1;
        let tmp = std::env::temp_dir().join(format!(
            "asset-tool-{}-{}.png",
            std::process::id(),
            self.counter
        ));
        self.renderer.read_back_png(&tmp)?;
        let img = image::open(&tmp)?.to_rgba8();
        let _ = std::fs::remove_file(&tmp);
        debug_assert_eq!(img.dimensions(), (self.size, self.size));
        Ok(img)
    }
}

/// Renders the thumbnail of one recipe.
pub fn thumbnail(
    t: &mut Thumbnailer,
    recipe: &Recipe,
    palette: &Palette,
) -> Result<image::RgbaImage> {
    let mesh = build_mesh(recipe, palette, None)?;
    t.render(&mesh)
}

/// Lays thumbnails out in a grid (left to right, top to bottom).
pub fn contact_sheet(thumbs: &[image::RgbaImage], columns: u32) -> image::RgbaImage {
    let columns = columns.clamp(1, (thumbs.len() as u32).max(1));
    let rows = (thumbs.len() as u32).div_ceil(columns).max(1);
    let (w, h) = thumbs
        .first()
        .map_or((THUMB_SIZE, THUMB_SIZE), |t| t.dimensions());
    let mut sheet =
        image::RgbaImage::from_pixel(columns * w, rows * h, image::Rgba([20, 20, 24, 255]));
    for (i, t) in thumbs.iter().enumerate() {
        let (cx, cy) = (i as u32 % columns, i as u32 / columns);
        image::imageops::replace(&mut sheet, t, i64::from(cx * w), i64::from(cy * h));
    }
    sheet
}
