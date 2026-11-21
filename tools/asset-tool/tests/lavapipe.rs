//! Thumbnail render tests on lavapipe (run by `cargo xtask render-test`).

use std::path::{Path, PathBuf};

use asset_tool::{contact_sheet, thumbnail, Thumbnailer, THUMB_SIZE};
use faset_assets::{Palette, Recipe};
use image::GenericImageView;

const PROPS: [&str; 2] = ["crate_wood", "tree_pine"];

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn recipe(id: &str) -> Recipe {
    let path = manifest().join(format!("../../assets/props/reference/{id}.ron"));
    Recipe::from_ron(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn reference(id: &str) -> PathBuf {
    manifest().join(format!("tests/refs/{id}.png"))
}

fn render(t: &mut Thumbnailer, id: &str) -> image::RgbaImage {
    thumbnail(t, &recipe(id), &Palette::default()).unwrap()
}

fn max_channel_error(a: &image::RgbaImage, b: &image::RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions(), "image sizes differ");
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn thumbnails_match_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    for id in PROPS {
        let got = render(&mut t, id);
        let want = image::open(reference(id)).unwrap().to_rgba8();
        let error = max_channel_error(&got, &want);
        assert!(error <= 1, "{id}: max channel error {error} > 1");
    }
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

#[test]
fn contact_sheet_tiles_thumbnails() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    let thumbs: Vec<_> = PROPS.iter().map(|id| render(&mut t, id)).collect();
    let sheet = contact_sheet(&thumbs, 5);
    assert_eq!(sheet.dimensions(), (2 * THUMB_SIZE, THUMB_SIZE));
    assert_eq!(
        sheet.view(0, 0, THUMB_SIZE, THUMB_SIZE).to_image(),
        thumbs[0]
    );
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

/// Regenerates `tests/refs/*.png`. Inspect the images before committing.
#[test]
#[ignore = "regenerates tests/refs"]
fn bless_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    for id in PROPS {
        render(&mut t, id).save(reference(id)).unwrap();
    }
}

// --- lot 2.15: characters, weapons and procedural animation ---------------

use faset_assets::anim::{limb_pose, Pose};
use faset_assets::character::Rig;
use faset_assets::viewmodel::{RecoilPattern, Viewmodel, ViewmodelInputs};
use faset_render::glam::{Mat4, Vec3};

const RUN_PHASES: [f32; 3] = [0.0, 1.9, 4.4];
const RUN_SPEED: f32 = 5.0;

fn load(dir: &str, id: &str) -> Recipe {
    let path = manifest().join(format!("../../assets/{dir}/{id}.ron"));
    Recipe::from_ron(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn mean_error(a: &image::RgbaImage, b: &image::RgbaImage) -> f32 {
    let total: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    total as f32 / a.as_raw().len() as f32
}

/// The running soldier seen from the side at three stride phases.
fn running_frames(t: &mut Thumbnailer) -> Vec<image::RgbaImage> {
    let palette = Palette::default();
    let rig = Rig::from_recipe(&load("characters", "soldier"), &palette).unwrap();
    RUN_PHASES
        .iter()
        .map(|&phase| {
            let transforms = rig.part_transforms(&limb_pose(RUN_SPEED, phase));
            let parts: Vec<_> = rig
                .parts
                .iter()
                .zip(transforms)
                .map(|(p, m)| (&p.mesh, m))
                .collect();
            t.render_scene(
                &parts,
                Vec3::new(3.2, 1.0, 0.0),
                Vec3::new(0.0, 0.9, 0.0),
                35.0,
                0.05,
            )
            .unwrap()
        })
        .collect()
}

/// The first-person rifle at rest and one frame after a shot.
fn weapon_frames(t: &mut Thumbnailer) -> Vec<image::RgbaImage> {
    let mesh = build_mesh(&load("weapons", "test_rifle"), &Palette::default(), None).unwrap();
    let rest_place = Mat4::from_translation(Vec3::new(0.13, -0.17, -0.3));
    let idle = ViewmodelInputs {
        grounded: true,
        ..Default::default()
    };
    let mut vm = Viewmodel::new(1);
    let at_rest = vm.update(1.0 / 60.0, &idle);
    vm.on_shot(&RecoilPattern::test_rifle());
    let shot = vm.update(1.0 / 60.0, &idle);
    [at_rest, shot]
        .iter()
        .map(|off| {
            t.render_scene(
                &[(&mesh, rest_place * off.to_mat4())],
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -1.0),
                60.0,
                0.02,
            )
            .unwrap()
        })
        .collect()
}

use faset_assets::build_mesh;

#[test]
fn running_character_poses_are_distinct_and_match_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    let frames = running_frames(&mut t);
    for i in 0..frames.len() {
        for j in i + 1..frames.len() {
            let d = mean_error(&frames[i], &frames[j]);
            assert!(d > 0.4, "phases {i} and {j} look alike (mean error {d})");
        }
        let want = image::open(reference(&format!("soldier_run_{i}")))
            .unwrap()
            .to_rgba8();
        assert!(max_channel_error(&frames[i], &want) <= 1, "run frame {i}");
    }
    let neutral = {
        let rig = Rig::from_recipe(&load("characters", "soldier"), &Palette::default()).unwrap();
        rig.part_transforms(&Pose::NEUTRAL)
    };
    assert!(neutral.iter().all(|m| m.is_finite()));
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

#[test]
fn weapon_frame_after_a_shot_shows_the_recoil() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    let frames = weapon_frames(&mut t);
    let d = mean_error(&frames[0], &frames[1]);
    assert!(d > 0.2, "recoil is not visible (mean error {d})");
    for (i, name) in ["rifle_rest", "rifle_shot"].iter().enumerate() {
        let want = image::open(reference(name)).unwrap().to_rgba8();
        assert!(max_channel_error(&frames[i], &want) <= 1, "{name}");
    }
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

/// Regenerates the lot 2.15 references. Inspect the images before committing.
#[test]
#[ignore = "regenerates tests/refs"]
fn bless_animation_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    for (i, f) in running_frames(&mut t).iter().enumerate() {
        f.save(reference(&format!("soldier_run_{i}"))).unwrap();
    }
    for (name, f) in ["rifle_rest", "rifle_shot"]
        .iter()
        .zip(weapon_frames(&mut t))
    {
        f.save(reference(name)).unwrap();
    }
}
