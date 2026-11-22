//! The asset library the editor places from: prop recipes with headless
//! thumbnails and real bounds, brush materials, colour palettes and the
//! ambience, sky and weather names a theme can use.
//!
//! Thumbnails are rendered on the CPU by a tiny z-buffer rasteriser (no GPU
//! needed, so they are produced the same way in tests and in the editor).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use faset_assets::{build_mesh, Mesh, Palette, Recipe};
use faset_core::glam::Vec3;

use crate::element::PropBounds;
use crate::math::Aabb;

/// Side of a thumbnail in pixels (decision: 64).
pub const THUMBNAIL_SIZE: usize = 64;

/// Brush materials known to the map geometry (`faset_mapfmt::geometry`).
pub const MATERIALS: [&str; 16] = [
    "concrete",
    "metal",
    "container_red",
    "container_blue",
    "sand",
    "checker",
    "hazard",
    "grid",
    "brick",
    "wood",
    "tiles",
    "snow",
    "water",
    "lava",
    "emissive",
    "hologram",
];

/// Colour palette names a map can select (decision: one per prop category;
/// the renderer resolves the name, thumbnails use the neutral default
/// palette).
pub const PALETTES: [&str; 9] = [
    "industrial",
    "desert",
    "snow",
    "urban",
    "nature",
    "neon",
    "space",
    "underwater",
    "interior",
];

/// Sky preset names (decision: the plan does not list them).
pub const SKIES: [&str; 5] = ["clear", "overcast", "dusk", "night", "stars"];

/// Weather names of the calendar; they follow the renderer's weather kinds.
pub const WEATHERS: [&str; 5] = ["clear", "rain", "snow", "storm", "sand"];

/// Ambience profile names: the renderer's built-in profiles.
pub use faset_render::BUILTIN_PROFILES as AMBIENCES;

/// A square RGBA image, straight alpha, row 0 at the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub size: usize,
    pub rgba: Vec<u8>,
}

impl Thumbnail {
    /// Number of pixels that were drawn on (alpha not zero).
    #[must_use]
    pub fn covered_pixels(&self) -> usize {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] != 0)
            .count()
    }
}

/// One placeable prop.
#[derive(Debug, Clone)]
pub struct PropEntry {
    pub id: String,
    pub tags: Vec<String>,
    /// Local bounds of the built mesh.
    pub bounds: Aabb,
    pub triangles: usize,
    pub thumbnail: Thumbnail,
}

/// Why a recipe file did not make it into the library.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Recipe { path: PathBuf, message: String },
}

/// The props available to the editor.
#[derive(Debug, Clone, Default)]
pub struct Library {
    props: BTreeMap<String, PropEntry>,
}

impl Library {
    /// An empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the library from recipes. A recipe that does not build is
    /// reported and skipped.
    #[must_use]
    pub fn from_recipes(recipes: impl IntoIterator<Item = Recipe>) -> (Self, Vec<LibraryError>) {
        let mut lib = Self::new();
        let mut errors = Vec::new();
        let palette = Palette::default_theme();
        for recipe in recipes {
            match lib.add_recipe(&recipe, &palette) {
                Ok(()) => {}
                Err(message) => errors.push(LibraryError::Recipe {
                    path: PathBuf::from(&recipe.id),
                    message,
                }),
            }
        }
        (lib, errors)
    }

    fn add_recipe(&mut self, recipe: &Recipe, palette: &Palette) -> Result<(), String> {
        let mesh = build_mesh(recipe, palette, None).map_err(|e| e.to_string())?;
        let (lo, hi) = mesh.bounds().ok_or("empty mesh")?;
        self.props.insert(
            recipe.id.clone(),
            PropEntry {
                id: recipe.id.clone(),
                tags: recipe.tags.clone(),
                bounds: Aabb { min: lo, max: hi },
                triangles: mesh.triangle_count(),
                thumbnail: render_thumbnail(&mesh, THUMBNAIL_SIZE),
            },
        );
        Ok(())
    }

    /// Loads every `*.ron` recipe under `dir`, recursively. Files that do not
    /// parse or build are returned as errors and skipped.
    #[must_use]
    pub fn load_dir(dir: &Path) -> (Self, Vec<LibraryError>) {
        let mut files = Vec::new();
        let mut errors = Vec::new();
        collect_ron(dir, &mut files, &mut errors);
        files.sort();
        let mut recipes = Vec::new();
        for path in files {
            match std::fs::read_to_string(&path) {
                Ok(text) => match Recipe::from_ron(&text) {
                    Ok(r) => recipes.push(r),
                    Err(e) => errors.push(LibraryError::Recipe {
                        path,
                        message: e.to_string(),
                    }),
                },
                Err(source) => errors.push(LibraryError::Io { path, source }),
            }
        }
        let (lib, mut build_errors) = Self::from_recipes(recipes);
        errors.append(&mut build_errors);
        (lib, errors)
    }

    /// Number of props.
    #[must_use]
    pub fn len(&self) -> usize {
        self.props.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// A prop by recipe id.
    #[must_use]
    pub fn prop(&self, id: &str) -> Option<&PropEntry> {
        self.props.get(id)
    }

    /// All props, ordered by id.
    pub fn props(&self) -> impl Iterator<Item = &PropEntry> {
        self.props.values()
    }

    /// Props whose id or a tag contains `query` (case-insensitive); an empty
    /// query matches everything.
    pub fn search<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a PropEntry> {
        let q = query.to_lowercase();
        self.props.values().filter(move |p| {
            q.is_empty()
                || p.id.to_lowercase().contains(&q)
                || p.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
    }

    /// Prop bounds for picking and handles.
    #[must_use]
    pub fn prop_bounds(&self) -> LibraryBounds {
        LibraryBounds(
            self.props
                .iter()
                .map(|(id, p)| (id.clone(), p.bounds))
                .collect(),
        )
    }
}

fn collect_ron(dir: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<LibraryError>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(source) => {
            errors.push(LibraryError::Io {
                path: dir.to_path_buf(),
                source,
            });
            return;
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_ron(&path, files, errors);
        } else if path.extension().is_some_and(|e| e == "ron") {
            files.push(path);
        }
    }
}

/// Real prop bounds taken from the library; unknown recipes get a 1 m cube.
#[derive(Debug, Clone, Default)]
pub struct LibraryBounds(BTreeMap<String, Aabb>);

impl PropBounds for LibraryBounds {
    fn bounds(&self, recipe: &str) -> Aabb {
        self.0.get(recipe).copied().unwrap_or(Aabb {
            min: Vec3::new(-0.5, 0.0, -0.5),
            max: Vec3::new(0.5, 1.0, 0.5),
        })
    }
}

/// Renders `mesh` from a three-quarter view into a `size` x `size` image.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn render_thumbnail(mesh: &Mesh, size: usize) -> Thumbnail {
    let mut rgba = vec![0_u8; size * size * 4];
    let Some((lo, hi)) = mesh.bounds() else {
        return Thumbnail { size, rgba };
    };
    let centre = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1e-3);
    let forward = Vec3::new(-1.0, -0.8, -1.0).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let light = Vec3::new(-0.4, 0.8, 0.5).normalize();
    let half = size as f32 * 0.5;
    let scale = half * 0.92 / radius;
    let project = |p: Vec3| {
        let d = p - centre;
        (
            half + d.dot(right) * scale,
            half - d.dot(up) * scale,
            d.dot(forward),
        )
    };
    let mut depth = vec![f32::INFINITY; size * size];
    for tri in mesh.indices.as_chunks::<3>().0.iter() {
        let v = [0, 1, 2].map(|k| &mesh.vertices[tri[k] as usize]);
        let p = v.map(|v| project(Vec3::from_array(v.position)));
        let n = v[0].normal_vec().normalize_or_zero();
        let shade = 0.35 + 0.65 * n.dot(light).max(0.0);
        let c = v[0].color_material;
        let colour = [0, 1, 2].map(|k| (f32::from(c[k]) * shade).min(255.0) as u8);
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-6 {
            continue;
        }
        let min_x = p
            .iter()
            .map(|q| q.0)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
        let min_y = p
            .iter()
            .map(|q| q.1)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2;
                let i = y * size + x;
                if z < depth[i] {
                    depth[i] = z;
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
            }
        }
    }
    Thumbnail { size, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crate_recipe() -> Recipe {
        Recipe::from_ron(
            r#"Prop(id: "crate", tags: ["cover"], collision: Box, parts: [
                Box(size: (1.0, 1.0, 1.0), color: "wood")])"#,
        )
        .unwrap()
    }

    #[test]
    fn a_recipe_gets_bounds_and_a_non_empty_thumbnail() {
        let (lib, errors) = Library::from_recipes([crate_recipe()]);
        assert!(errors.is_empty());
        let p = lib.prop("crate").unwrap();
        assert!((p.bounds.max.y - p.bounds.min.y - 1.0).abs() < 1e-3);
        assert_eq!(p.thumbnail.size, THUMBNAIL_SIZE);
        let covered = p.thumbnail.covered_pixels();
        assert!(covered > 300 && covered < THUMBNAIL_SIZE * THUMBNAIL_SIZE);
        assert_eq!(lib.search("cov").count(), 1);
        assert_eq!(lib.search("zzz").count(), 0);
    }

    #[test]
    fn unknown_recipes_get_a_unit_cube() {
        let b = LibraryBounds::default().bounds("nope");
        assert_eq!(b.max.y, 1.0);
    }
}
