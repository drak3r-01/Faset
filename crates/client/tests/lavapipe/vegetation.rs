//! Vegetation on Mesa lavapipe (lot 6.11): every species drawn alone against
//! a reference contact sheet, and a dense field scattered from density rules
//! through the client's map pipeline (instanced, culled per cell, levels of
//! detail), with a frame-time benchmark. The validation layers must stay
//! silent.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use faset_assets::vegetation::plant_mesh;
use faset_client::load_map;
use faset_client::scenery::foliage_layer;
use faset_mapfmt::vegetation::PlacedPlant;
use faset_mapfmt::Species;
use faset_render::glam::Vec3;
use faset_render::{
    perspective_reverse_z, AmbienceProfile, Camera, RenderConfig, RenderWorld, Renderer,
};

const TILE: u32 = 192;
const SHEET_REFERENCE: &str = "tests/refs/vegetation_species.png";
const CLEAR: [f32; 4] = [0.25, 0.35, 0.5, 1.0];

fn look_at(eye: Vec3, target: Vec3, fov_deg: f32, aspect: f32) -> Camera {
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    Camera::new(
        view,
        perspective_reverse_z(fov_deg.to_radians(), aspect, 0.05),
    )
}

/// Renders every species (variant 0, `nature` theme) in its own tile and
/// assembles the tiles side by side. Returns the share of drawn pixels of
/// each tile.
fn render_species_sheet(path: &Path) -> Vec<f32> {
    let mut renderer = Renderer::new_headless(&RenderConfig {
        size: (TILE, TILE),
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    let plants: Vec<PlacedPlant> = Species::ALL
        .iter()
        .map(|&species| PlacedPlant {
            species,
            position: Vec3::ZERO,
            yaw: 0.4,
            scale: 1.0,
            variant: 0,
        })
        .collect();
    let (kinds, instances) =
        foliage_layer(&mut renderer, &plants, "nature").expect("plant meshes upload");
    let mut sheet = image::RgbaImage::new(TILE * Species::ALL.len() as u32, TILE);
    let mut coverage = Vec::new();
    for (k, species) in Species::ALL.iter().enumerate() {
        renderer
            .set_foliage(kinds.clone(), &instances[k..=k])
            .expect("one plant");
        // Frame the plant from its mesh bounds.
        let (lo, hi) = plant_mesh(*species, 0, 0, "nature")
            .unwrap()
            .bounds()
            .unwrap();
        let size = (hi - lo).max_element();
        let target = Vec3::new(0.0, (lo.y + hi.y) * 0.5, 0.0);
        let distance = 0.8 * size / 20f32.to_radians().tan();
        let eye = target + Vec3::new(0.35, 0.3, 1.0).normalize() * distance;
        let world = RenderWorld {
            clear_color: CLEAR,
            camera: look_at(eye, target, 40.0, 1.0),
            ambience: AmbienceProfile::builtin("day").unwrap(),
            foliage: true,
            ..RenderWorld::default()
        };
        renderer.set_material_time(0.0);
        for _ in 0..3 {
            renderer.render_frame(&world).unwrap();
        }
        assert_eq!(renderer.last_frame_stats().foliage_instances, 1);
        let tile = std::env::temp_dir().join(format!(
            "{}-plant-{}.png",
            std::process::id(),
            species.name()
        ));
        renderer.read_back_png(&tile).unwrap();
        let img = image::open(&tile).unwrap().to_rgba8();
        let _ = std::fs::remove_file(&tile);
        let clear = img.get_pixel(0, 0).0;
        let drawn = img.pixels().filter(|p| p.0 != clear).count();
        coverage.push(drawn as f32 / img.pixels().len() as f32);
        image::imageops::replace(&mut sheet, &img, i64::from(TILE) * k as i64, 0);
    }
    assert_eq!(
        renderer.validation_message_count(),
        0,
        "validation messages"
    );
    sheet.save(path).unwrap();
    coverage
}

fn sheet_reference() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(SHEET_REFERENCE)
}

#[test]
fn every_species_renders_and_matches_reference() {
    let out = std::env::temp_dir().join(format!("{}-vegetation_species.png", std::process::id()));
    let coverage = render_species_sheet(&out);
    for (species, c) in Species::ALL.iter().zip(&coverage) {
        assert!(
            *c > 0.02,
            "{species:?} covers {:.1} % of its tile",
            c * 100.0
        );
    }
    let got = image::open(&out).unwrap().to_rgba8();
    let reference = image::open(sheet_reference())
        .expect("reference image")
        .to_rgba8();
    assert_eq!(got.dimensions(), reference.dimensions());
    let error = got
        .as_raw()
        .iter()
        .zip(reference.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        error <= 2,
        "max channel error {error} > 2 (output: {})",
        out.display()
    );
    let _ = std::fs::remove_file(out);
}

/// Regenerates the reference from a lavapipe render. Inspect before committing.
#[test]
#[ignore = "regenerates tests/refs/vegetation_species.png"]
fn bless_vegetation_species_reference() {
    render_species_sheet(&sheet_reference());
}

/// A 120 m x 120 m meadow: about 35 000 plants of six species from density
/// rules, through the client's map pipeline.
const MEADOW: &str = r#"Map(
    format_version: 3,
    meta: (name: "meadow", modes: ["fps"], players: (1, 2)),
    theme: (ambience: "day", palette: "nature"),
    brushes: [Box(min: (-120, -2, -120), max: (120, 0, 120), material: "grass")],
    vegetation: (seed: 5, rules: [
        (on: "grass", species: Grass, density: 200.0),
        (on: "grass", species: Fern, density: 20.0),
        (on: "grass", species: Bush, density: 2.0),
        (on: "grass", species: Oak, density: 0.8),
        (on: "grass", species: Pine, density: 0.8),
        (on: "grass", species: Rock, density: 1.0),
    ]),
)"#;

const FIELD_SIZE: (u32, u32) = (640, 360);

/// Median wall time of a frame over `frames` frames.
fn median_frame(renderer: &mut Renderer, world: &RenderWorld, frames: usize) -> Duration {
    let mut times: Vec<Duration> = (0..frames)
        .map(|_| {
            let start = Instant::now();
            renderer.render_frame(world).unwrap();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

#[test]
fn dense_field_is_instanced_culled_and_within_budget() {
    let map = load_map(MEADOW.as_bytes()).expect("the meadow loads");
    assert!(
        (30_000..=40_000).contains(&map.plants.len()),
        "{} plants",
        map.plants.len()
    );
    let mut renderer = Renderer::new_headless(&RenderConfig {
        size: FIELD_SIZE,
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    let instances = map.upload(&mut renderer).expect("the meadow uploads");
    let aspect = FIELD_SIZE.0 as f32 / FIELD_SIZE.1 as f32;
    let mut world = RenderWorld {
        clear_color: CLEAR,
        camera: look_at(
            Vec3::new(0.0, 1.7, 40.0),
            Vec3::new(0.0, 1.0, 0.0),
            75.0,
            aspect,
        ),
        instances,
        ambience: AmbienceProfile::builtin("day").unwrap(),
        draw_sky: true,
        foliage: true,
        ..RenderWorld::default()
    };
    renderer.set_material_time(2.0);
    for _ in 0..2 {
        renderer.render_frame(&world).unwrap();
    }
    let stats = renderer.last_frame_stats();
    // Thousands of plants for a few hundred draws at most; the plants behind
    // the camera and beyond their draw distance are not drawn.
    assert!(stats.foliage_instances > 3_000, "{stats:?}");
    assert!(
        (stats.foliage_instances as usize) < map.plants.len() / 2,
        "{stats:?}"
    );
    assert!(stats.foliage_draw_calls <= 600, "{stats:?}");
    if let Ok(dir) = std::env::var("FASET_MAP_SHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        renderer
            .read_back_png(&Path::new(&dir).join("dense-meadow.png"))
            .unwrap();
    }

    // Benchmark: the same view with and without the vegetation.
    let with = median_frame(&mut renderer, &world, 5);
    world.foliage = false;
    let without = median_frame(&mut renderer, &world, 5);
    println!(
        "dense meadow ({} plants, {} drawn in {} draws): {:.1} ms per frame with the \
         vegetation, {:.1} ms without (lavapipe, {}x{})",
        map.plants.len(),
        stats.foliage_instances,
        stats.foliage_draw_calls,
        with.as_secs_f64() * 1e3,
        without.as_secs_f64() * 1e3,
        FIELD_SIZE.0,
        FIELD_SIZE.1
    );
    // Software rasterisation under a loaded CI machine: a loose ceiling that
    // still catches a per-plant CPU path or a missing cull.
    assert!(with < Duration::from_secs(3), "{with:?} per frame");
    assert_eq!(
        renderer.validation_message_count(),
        0,
        "validation messages"
    );
}
