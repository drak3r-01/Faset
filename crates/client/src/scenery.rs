//! Map scenery (lot 6.11): the map's props and its vegetation, drawn.
//!
//! * Props: the recipes of `assets/props` are embedded in the executable
//!   ([`PropCatalog`]); each recipe a map uses is built once with the neutral
//!   palette and drawn as a static instance per prop.
//! * Vegetation: the plants of the map ([`faset_mapfmt::vegetation::scatter`],
//!   the same list the bots' sight uses) are drawn by the renderer's
//!   instanced foliage layer, with the code-generated meshes of
//!   `faset_assets::vegetation` in the map's theme.
//!
//! Decisions of lot 6.11:
//! - a prop stands on its position: its mesh is lifted so that its lowest
//!   point is at `at`, like the collision box the simulation gives it;
//! - a prop whose recipe is unknown is drawn as the 1 m box it collides as;
//! - the vegetation theme is the map palette name (`snow`, `desert`, else the
//!   `nature` look).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use faset_assets::vegetation::{look, plant_mesh, LODS};
use faset_assets::{build_mesh, Mesh, Palette, Recipe};
use faset_mapfmt::vegetation::{PlacedPlant, VARIANTS};
use faset_mapfmt::{MapFile, Prop, Species};
use faset_render::glam::{Mat4, Vec3};
use faset_render::{FoliageInstance, FoliageKind, Instance, MeshId, RenderError, Renderer, Vertex};

include!(concat!(env!("OUT_DIR"), "/props.rs"));

/// The prop recipes shipped with the game, by recipe id.
#[derive(Debug, Default)]
pub struct PropCatalog {
    recipes: BTreeMap<String, Recipe>,
}

impl PropCatalog {
    /// The embedded recipes (parsed once). A file that does not parse is
    /// skipped with a warning.
    #[must_use]
    pub fn embedded() -> &'static Self {
        static CATALOG: OnceLock<PropCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            let mut recipes = BTreeMap::new();
            for (path, text) in PROP_RECIPES {
                match Recipe::from_ron(text) {
                    Ok(r) => {
                        recipes.entry(r.id.clone()).or_insert(r);
                    }
                    Err(error) => tracing::warn!(path, %error, "prop recipe skipped"),
                }
            }
            Self { recipes }
        })
    }

    /// A recipe by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.recipes.get(id)
    }

    /// Number of recipes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

/// The 1 m box an unknown prop collides as, base at the origin.
fn placeholder_box() -> Mesh {
    let recipe = Recipe::from_ron(
        r#"Prop(id: "placeholder", parts: [Box(size: (1.0, 1.0, 1.0), at: (0.0, 0.5, 0.0), color: "wood", pattern: PixelCrate)])"#,
    )
    .expect("valid placeholder recipe");
    build_mesh(&recipe, &Palette::default_theme(), None).expect("placeholder builds")
}

/// The mesh of a prop recipe id (neutral palette), or the placeholder box.
#[must_use]
pub fn prop_mesh(id: &str) -> Mesh {
    PropCatalog::embedded()
        .get(id)
        .and_then(|r| match build_mesh(r, &Palette::default_theme(), None) {
            Ok(m) if !m.indices.is_empty() => Some(m),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(recipe = id, %error, "prop does not build");
                None
            }
        })
        .unwrap_or_else(|| {
            tracing::debug!(recipe = id, "unknown prop, drawn as a box");
            placeholder_box()
        })
}

/// World matrix of a prop whose mesh's lowest point is `base_y` (local).
#[must_use]
pub fn prop_matrix(prop: &Prop, base_y: f32) -> Mat4 {
    let s = if prop.scale > 0.0 { prop.scale } else { 1.0 };
    Mat4::from_translation(Vec3::from_array(prop.at))
        * Mat4::from_rotation_y(prop.yaw.to_radians())
        * Mat4::from_scale(Vec3::splat(s))
        * Mat4::from_translation(Vec3::new(0.0, -base_y, 0.0))
}

/// The plant theme of a map palette.
#[must_use]
pub fn plant_theme(palette: &str) -> &'static str {
    match palette {
        "snow" => "snow",
        "desert" => "desert",
        _ => "nature",
    }
}

fn to_render(mesh: &Mesh) -> Vec<Vertex> {
    mesh.vertices
        .iter()
        .map(|v| Vertex {
            position: v.position,
            normal: v.normal,
            color_material: v.color_material,
        })
        .collect()
}

/// The foliage kind index of a plant: species then variant.
#[must_use]
pub fn kind_index(species: Species, variant: u32) -> u32 {
    species.index() as u32 * VARIANTS + variant.min(VARIANTS - 1)
}

/// Uploads the meshes of every plant kind used by `plants` and builds the
/// foliage layer: kinds (indexed by [`kind_index`]; unused kinds get no
/// mesh and an empty level list is avoided by uploading every kind of a
/// used species) and instances.
///
/// # Errors
/// A mesh upload failed.
pub fn foliage_layer(
    renderer: &mut Renderer,
    plants: &[PlacedPlant],
    theme: &str,
) -> Result<(Vec<FoliageKind>, Vec<FoliageInstance>), RenderError> {
    let mut used = [false; Species::ALL.len()];
    for p in plants {
        used[p.species.index()] = true;
    }
    let mut kinds = Vec::with_capacity(Species::ALL.len() * VARIANTS as usize);
    // A one-triangle stand-in for kinds no plant uses, so indices stay
    // `kind_index`.
    let mut stand_in: Option<MeshId> = None;
    for species in Species::ALL {
        let l = look(species);
        for variant in 0..VARIANTS {
            let lods = if used[species.index()] {
                let mut lods = Vec::with_capacity(LODS);
                for (lod, distance) in l.lod_distances.iter().enumerate() {
                    let mesh = plant_mesh(species, variant, lod, theme).map_err(|e| {
                        RenderError::InvalidFoliage(format!("{species:?} does not build: {e}"))
                    })?;
                    lods.push((
                        renderer.upload_mesh(&to_render(&mesh), &mesh.indices)?,
                        *distance,
                    ));
                }
                lods
            } else {
                let id = match stand_in {
                    Some(id) => id,
                    None => {
                        let v = Vertex::new(Vec3::ZERO, Vec3::Y, [0; 3], 0);
                        let id = renderer.upload_mesh(&[v; 3], &[0, 1, 2])?;
                        stand_in = Some(id);
                        id
                    }
                };
                vec![(id, 1.0)]
            };
            kinds.push(FoliageKind {
                lods,
                sway: l.sway,
                casts_shadow: l.casts_shadow,
            });
        }
    }
    let instances = plants
        .iter()
        .map(|p| FoliageInstance {
            kind: kind_index(p.species, p.variant),
            position: p.position,
            yaw: p.yaw,
            scale: p.scale,
        })
        .collect();
    Ok((kinds, instances))
}

/// Uploads the props of an expanded map and returns their instances.
///
/// # Errors
/// A mesh upload failed.
pub fn prop_instances(
    renderer: &mut Renderer,
    map: &MapFile,
) -> Result<Vec<Instance>, RenderError> {
    let mut meshes: HashMap<&str, (MeshId, f32)> = HashMap::new();
    let mut out = Vec::with_capacity(map.props.len());
    for prop in &map.props {
        let (mesh, base) = match meshes.get(prop.recipe.as_str()) {
            Some(m) => *m,
            None => {
                let built = prop_mesh(&prop.recipe);
                let base = built.bounds().map_or(0.0, |(lo, _)| lo.y);
                let id = renderer.upload_mesh(&to_render(&built), &built.indices)?;
                meshes.insert(prop.recipe.as_str(), (id, base));
                (id, base)
            }
        };
        out.push(Instance::new(mesh, 0, prop_matrix(prop, base)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_recipe_is_embedded_and_builds() {
        let catalog = PropCatalog::embedded();
        assert!(catalog.len() >= 150, "{} recipes", catalog.len());
        assert!(catalog.get("crate_wood").is_some());
        for (path, text) in PROP_RECIPES {
            let r = Recipe::from_ron(text).unwrap_or_else(|e| panic!("{path}: {e}"));
            let mesh = build_mesh(&r, &Palette::default_theme(), None)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!mesh.indices.is_empty(), "{path}");
        }
    }

    #[test]
    fn props_stand_on_their_position() {
        let prop = Prop {
            recipe: "crate_wood".into(),
            at: [2.0, 1.0, -3.0],
            yaw: 90.0,
            scale: 2.0,
        };
        let mesh = prop_mesh(&prop.recipe);
        let (lo, _) = mesh.bounds().unwrap();
        let m = prop_matrix(&prop, lo.y);
        let lowest = mesh
            .vertices
            .iter()
            .map(|v| m.transform_point3(Vec3::from_array(v.position)).y)
            .fold(f32::INFINITY, f32::min);
        assert!((lowest - 1.0).abs() < 1e-4, "base at {lowest}");
        // Unknown recipes are the 1 m box they collide as.
        let b = prop_mesh("no_such_prop");
        let (lo, hi) = b.bounds().unwrap();
        assert!((hi - lo - Vec3::ONE).abs().max_element() < 1e-5);
        assert!(lo.y.abs() < 1e-6);
    }

    #[test]
    fn shipped_maps_have_vegetation_off_their_spawns() {
        for name in ["driftwood-cove", "red-mesa", "pine-hollow", "rust-harbor"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/maps")
                .join(format!("{name}.ron"));
            let map = crate::load_map(&std::fs::read(path).unwrap()).unwrap();
            assert!(
                (20..=20_000).contains(&map.plants.len()),
                "{name}: {} plants",
                map.plants.len()
            );
            for spawn in &map.map.spawns {
                for p in &map.plants {
                    let d = (p.position.x - spawn.at[0]).hypot(p.position.z - spawn.at[2]);
                    assert!(d >= faset_mapfmt::vegetation::CLEARANCE_M - 1e-4, "{name}");
                }
            }
        }
    }

    #[test]
    fn kinds_are_indexed_by_species_then_variant() {
        assert_eq!(kind_index(Species::Oak, 0), 0);
        assert_eq!(kind_index(Species::Pine, 2), VARIANTS + 2);
        assert_eq!(
            kind_index(Species::Fern, VARIANTS - 1),
            Species::ALL.len() as u32 * VARIANTS - 1
        );
        assert_eq!(plant_theme("snow"), "snow");
        assert_eq!(plant_theme("industrial"), "nature");
    }
}
