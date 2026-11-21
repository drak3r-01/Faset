//! Headless upscaler tests on Mesa lavapipe (run by `cargo xtask render-test`).
//!
//! Every test asserts zero validation messages, counted twice: by the
//! renderer's debug messenger, and by a tracing layer that also sees messages
//! emitted while the device and instance are destroyed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    cube_mesh, perspective_reverse_z, AmbienceProfile, Camera, Instance, MeshId, PostSettings,
    RenderConfig, RenderWorld, Renderer, SkyParams, WaterParams, WeatherState, CUBE_FACE_TINTS,
};
use faset_upscale::{scaled_extent, Extent2D, Fsr1, Native, Quality, Registry, Taa, UpscalerKind};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// Display size of the reference images.
const DISPLAY: (u32, u32) = (256, 144);

/// Counts events logged by the renderer's Vulkan debug callback.
struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn capture_messages() -> (Arc<AtomicUsize>, DefaultGuard) {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(VulkanMessages(count.clone()))
        .with(tracing_subscriber::fmt::layer().with_test_writer());
    (count, tracing::subscriber::set_default(subscriber))
}

fn headless(size: (u32, u32)) -> Renderer {
    let renderer = Renderer::new_headless(&RenderConfig {
        size,
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    assert!(
        renderer.validation_enabled(),
        "validation layer must be active in test builds"
    );
    renderer
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("faset-upscale-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
}

fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

fn load_rgba(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()))
        .to_rgba8()
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

/// Mean absolute error over the RGB channels, in 8-bit units.
fn mean_error(a: &image::RgbaImage, b: &image::RgbaImage) -> f32 {
    assert_eq!(a.dimensions(), b.dimensions(), "image sizes differ");
    let (sum, n) = a
        .pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..3).map(move |i| f32::from(p.0[i].abs_diff(q.0[i]))))
        .fold((0.0f32, 0u32), |(s, n), e| (s + e, n + 1));
    sum / n as f32
}

fn look_at(eye: Vec3, target: Vec3, size: (u32, u32)) -> Camera {
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    let aspect = size.0 as f32 / size.1 as f32;
    Camera::new(view, perspective_reverse_z(60f32.to_radians(), aspect, 0.1))
}

/// The meshes of the low-poly scene.
struct Meshes {
    ground: MeshId,
    cube: MeshId,
}

fn meshes(renderer: &mut Renderer) -> Meshes {
    let mut mesh = |tints: &[[u8; 3]; 6], material: u8| {
        let (v, i) = cube_mesh(0.5, tints, material);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    Meshes {
        ground: mesh(&[[200; 3]; 6], 2),
        cube: mesh(&CUBE_FACE_TINTS, 0),
    }
}

/// Ground, a few cubes of different sizes and a pillar, below a day sky.
fn lowpoly_instances(m: &Meshes) -> Vec<Instance> {
    let at = |x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32| {
        Mat4::from_translation(Vec3::new(x, y, z)) * Mat4::from_scale(Vec3::new(sx, sy, sz))
    };
    let mut instances = vec![Instance::new(
        m.ground,
        0,
        at(0.0, -0.5, 0.0, 200.0, 1.0, 200.0),
    )];
    for (x, y, z, sx, sy, sz) in [
        (-3.0, 0.5, -1.0, 1.0, 1.0, 1.0),
        (-0.5, 0.75, -3.0, 1.5, 1.5, 1.5),
        (2.5, 0.5, 0.0, 1.0, 1.0, 1.0),
        (4.5, 1.5, -4.0, 1.0, 3.0, 1.0),
        (0.5, 0.3, 2.0, 0.6, 0.6, 0.6),
        (-5.0, 1.0, -6.0, 2.0, 2.0, 2.0),
    ] {
        instances.push(Instance::new(m.cube, 0, at(x, y, z, sx, sy, sz)));
    }
    instances
}

fn day_ambience() -> AmbienceProfile {
    AmbienceProfile::builtin("day").expect("built-in profile")
}

fn lowpoly_world(instances: Vec<Instance>, camera: Camera) -> RenderWorld {
    RenderWorld {
        camera,
        instances,
        ambience: day_ambience(),
        sky: SkyParams {
            time_h: 13.0,
            latitude_deg: 45.0,
            ..SkyParams::default()
        },
        draw_sky: true,
        ..RenderWorld::default()
    }
}

fn pass(
    renderer: &Renderer,
    kind: UpscalerKind,
    quality: Quality,
) -> (UpscalerKind, Box<dyn faset_render::UpscalePass>) {
    Registry::builtin()
        .create_pass(kind, quality, &renderer.gpu_handles())
        .expect("an upscaler on lavapipe")
}

/// The low-poly scene through `kind` at `quality`, `frames` frames of a static
/// camera (the jitter still moves), with the post chain on.
fn render_lowpoly(kind: UpscalerKind, quality: Quality, frames: u32, path: &Path) {
    let mut renderer = headless(DISPLAY);
    renderer.set_material_time(1.25);
    let (built, p) = pass(&renderer, kind, quality);
    assert_eq!(built, kind);
    renderer.set_upscaler(Some(p)).unwrap();
    renderer
        .set_post_settings(Some(PostSettings::default()))
        .unwrap();
    let m = meshes(&mut renderer);
    let mut camera = look_at(Vec3::new(1.0, 2.6, 9.0), Vec3::new(0.0, 0.8, 0.0), DISPLAY);
    let world = lowpoly_world(lowpoly_instances(&m), camera);
    for frame in 0..frames {
        camera.jitter_px = renderer.upscaler_jitter(frame);
        renderer
            .render_frame(&RenderWorld {
                camera,
                ..world.clone()
            })
            .unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

const FSR1_SHOTS: [(&str, Quality); 4] = [
    ("fsr1_quality", Quality::Quality),
    ("fsr1_balanced", Quality::Balanced),
    ("fsr1_performance", Quality::Performance),
    ("fsr1_ultra_performance", Quality::UltraPerformance),
];

#[test]
fn fsr1_quality_levels_match_references() {
    let (messages, _guard) = capture_messages();
    let mut failures = Vec::new();
    for (name, quality) in FSR1_SHOTS {
        let out = out_path(&format!("{name}.png"));
        // Twice the frames of the two in flight, so every slot is reused.
        render_lowpoly(UpscalerKind::Fsr1, quality, 3, &out);
        let error = max_channel_error(&load_rgba(&out), &load_rgba(&reference(name)));
        if error > 2 {
            failures.push(format!(
                "{name}: max channel error {error} (output: {})",
                out.display()
            ));
        } else {
            let _ = std::fs::remove_file(out);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Regenerates the FSR 1 and TAA references from lavapipe renders. Inspect
/// the images before committing them.
#[test]
#[ignore = "regenerates tests/refs/*.png"]
fn bless_references() {
    for (name, quality) in FSR1_SHOTS {
        render_lowpoly(UpscalerKind::Fsr1, quality, 3, &reference(name));
    }
    render_lowpoly(
        UpscalerKind::Native,
        Quality::Native,
        3,
        &reference("native"),
    );
    moving_camera(true, &reference("taa_moving"));
}

#[test]
fn fsr1_sharpens_and_differs_from_a_plain_stretch() {
    let (messages, _guard) = capture_messages();
    let fsr = out_path("fsr_vs_stretch_a.png");
    let stretch = out_path("fsr_vs_stretch_b.png");
    render_lowpoly(UpscalerKind::Fsr1, Quality::Performance, 1, &fsr);
    render_lowpoly(UpscalerKind::Native, Quality::Free(0.5), 1, &stretch);
    let (a, b) = (load_rgba(&fsr), load_rgba(&stretch));
    assert!(
        max_channel_error(&a, &b) > 8,
        "EASU and RCAS leave the image as a bilinear stretch"
    );
    // Same scene, same resolution: they still look alike.
    assert!(mean_error(&a, &b) < 12.0, "{}", mean_error(&a, &b));
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn the_scene_is_drawn_at_the_internal_resolution() {
    let (messages, _guard) = capture_messages();
    let display = Extent2D {
        width: DISPLAY.0,
        height: DISPLAY.1,
    };
    for quality in Quality::PRESETS {
        let mut renderer = headless(DISPLAY);
        let (_, p) = pass(&renderer, UpscalerKind::Fsr1, quality);
        renderer.set_upscaler(Some(p)).unwrap();
        let expected = scaled_extent(display, quality);
        assert_eq!(
            renderer.internal_extent(DISPLAY),
            (expected.width, expected.height)
        );
        let m = meshes(&mut renderer);
        let camera = look_at(Vec3::new(1.0, 2.6, 9.0), Vec3::new(0.0, 0.8, 0.0), DISPLAY);
        renderer
            .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
            .unwrap();
        // The motion vectors have the internal size; the image the display's.
        let motion = renderer.read_back_motion().unwrap();
        assert_eq!(motion.len(), (expected.width * expected.height) as usize);
        let out = out_path("internal.png");
        renderer.read_back_png(&out).unwrap();
        assert_eq!(load_rgba(&out).dimensions(), DISPLAY);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn free_render_scale_is_clamped_to_fifty_percent() {
    let (messages, _guard) = capture_messages();
    let mut renderer = headless(DISPLAY);
    let (_, p) = pass(&renderer, UpscalerKind::Native, Quality::Free(0.1));
    renderer.set_upscaler(Some(p)).unwrap();
    assert_eq!(
        renderer.internal_extent(DISPLAY),
        (DISPLAY.0 / 2, DISPLAY.1 / 2)
    );
    let (_, p) = pass(&renderer, UpscalerKind::Fsr1, Quality::Free(0.75));
    renderer.set_upscaler(Some(p)).unwrap();
    assert_eq!(renderer.internal_extent(DISPLAY), (192, 108));
    // TAA never renders below the display size.
    let (_, p) = pass(&renderer, UpscalerKind::Taa, Quality::Free(0.5));
    renderer.set_upscaler(Some(p)).unwrap();
    assert_eq!(renderer.internal_extent(DISPLAY), DISPLAY);
    renderer.set_upscaler(None).unwrap();
    assert_eq!(renderer.internal_extent(DISPLAY), DISPLAY);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn every_builtin_upscaler_is_available_on_lavapipe() {
    let (messages, _guard) = capture_messages();
    let renderer = headless((64, 64));
    let registry = Registry::builtin();
    let modes = registry.available(&renderer.gpu_handles());
    assert_eq!(modes, UpscalerKind::ALL);
    for kind in UpscalerKind::ALL {
        let created = registry.create(kind, &renderer.gpu_handles()).unwrap();
        assert_eq!(created.kind, kind);
        assert!(!created.fell_back());
    }
    // The concrete types build on their own too.
    let handles = renderer.gpu_handles();
    drop((
        Native::new(&handles).unwrap(),
        Taa::new(&handles).unwrap(),
        Fsr1::new(&handles).unwrap(),
    ));
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn native_at_full_scale_leaves_the_image_unchanged() {
    let (messages, _guard) = capture_messages();
    // Without the post chain: pixels as the scene drew them.
    let direct = out_path("native_direct.png");
    {
        let mut renderer = headless(DISPLAY);
        renderer.set_material_time(1.25);
        let m = meshes(&mut renderer);
        let camera = look_at(Vec3::new(1.0, 2.6, 9.0), Vec3::new(0.0, 0.8, 0.0), DISPLAY);
        renderer
            .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
            .unwrap();
        renderer.read_back_png(&direct).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    let through = out_path("native_through.png");
    {
        let mut renderer = headless(DISPLAY);
        renderer.set_material_time(1.25);
        let (_, p) = pass(&renderer, UpscalerKind::Native, Quality::Native);
        renderer.set_upscaler(Some(p)).unwrap();
        let m = meshes(&mut renderer);
        let camera = look_at(Vec3::new(1.0, 2.6, 9.0), Vec3::new(0.0, 0.8, 0.0), DISPLAY);
        renderer
            .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
            .unwrap();
        renderer.read_back_png(&through).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(
        max_channel_error(&load_rgba(&direct), &load_rgba(&through)),
        0
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn upscalers_survive_resizes_and_switches() {
    let (messages, _guard) = capture_messages();
    let mut renderer = headless((128, 72));
    renderer
        .set_post_settings(Some(PostSettings::default()))
        .unwrap();
    let m = meshes(&mut renderer);
    let mut frame = 0;
    for kind in [
        UpscalerKind::Fsr1,
        UpscalerKind::Taa,
        UpscalerKind::Native,
        UpscalerKind::Fsr1,
    ] {
        let (_, p) = pass(&renderer, kind, Quality::Balanced);
        renderer.set_upscaler(Some(p)).unwrap();
        for size in [(128, 72), (200, 112), (96, 54), (128, 72)] {
            renderer.resize(size.0, size.1);
            let mut camera = look_at(Vec3::new(1.0, 2.6, 9.0), Vec3::new(0.0, 0.8, 0.0), size);
            for _ in 0..3 {
                camera.jitter_px = renderer.upscaler_jitter(frame);
                frame += 1;
                renderer
                    .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
                    .unwrap();
            }
        }
        let out = out_path("switch.png");
        renderer.read_back_png(&out).unwrap();
        assert_eq!(load_rgba(&out).dimensions(), (128, 72), "{kind:?}");
    }
    renderer.set_upscaler(None).unwrap();
    let camera = look_at(
        Vec3::new(1.0, 2.6, 9.0),
        Vec3::new(0.0, 0.8, 0.0),
        (128, 72),
    );
    renderer
        .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
        .unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    drop(renderer);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn weather_and_water_render_through_every_upscaler() {
    let (messages, _guard) = capture_messages();
    for kind in UpscalerKind::ALL {
        let mut renderer = headless((128, 72));
        renderer.set_material_time(3.0);
        renderer
            .set_post_settings(Some(PostSettings::default()))
            .unwrap();
        let (_, p) = pass(&renderer, kind, Quality::Balanced);
        renderer.set_upscaler(Some(p)).unwrap();
        let m = meshes(&mut renderer);
        let mut camera = look_at(
            Vec3::new(1.0, 2.6, 9.0),
            Vec3::new(0.0, 0.8, 0.0),
            (128, 72),
        );
        let mut world = lowpoly_world(lowpoly_instances(&m), camera);
        world.weather = WeatherState {
            rain: 1.0,
            ..WeatherState::CLEAR
        };
        world.water = Some(WaterParams {
            height: 0.2,
            waves: vec![faset_render::GerstnerWave {
                amplitude: 0.1,
                steepness: 0.5,
                direction: faset_render::glam::Vec2::X,
                wavelength: 8.0,
            }],
        });
        for frame in 0..3 {
            camera.jitter_px = renderer.upscaler_jitter(frame);
            world.camera = camera;
            renderer.render_frame(&world).unwrap();
        }
        assert_eq!(renderer.validation_message_count(), 0, "{kind:?}");
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

// ---------------------------------------------------------------------------
// Temporal anti-aliasing
// ---------------------------------------------------------------------------

const TAA_SIZE: (u32, u32) = (160, 90);

/// A TAA renderer with the post chain, the low-poly scene and a camera
/// sliding sideways; `motion` is whether the camera hands the renderer the
/// previous frame's matrix (without it, every motion vector is zero and the
/// history is never reprojected).
fn moving_camera(motion: bool, path: &Path) {
    let mut renderer = headless(TAA_SIZE);
    renderer.set_material_time(1.25);
    let (_, p) = pass(&renderer, UpscalerKind::Taa, Quality::Native);
    renderer.set_upscaler(Some(p)).unwrap();
    renderer
        .set_post_settings(Some(PostSettings::default()))
        .unwrap();
    let m = meshes(&mut renderer);
    let instances = lowpoly_instances(&m);
    let mut prev = None;
    for frame in 0..24u32 {
        let x = 0.06 * frame as f32;
        let eye = Vec3::new(x - 0.7, 2.6, 9.0);
        let mut camera = look_at(eye, eye + Vec3::new(-0.1, -1.8, -9.0), TAA_SIZE);
        camera.jitter_px = renderer.upscaler_jitter(frame);
        if motion {
            camera.prev_view_proj = prev.unwrap_or_else(|| camera.view_proj());
        } else {
            camera.prev_view_proj = camera.view_proj();
        }
        prev = Some(camera.view_proj());
        renderer
            .render_frame(&lowpoly_world(instances.clone(), camera))
            .unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

/// The same final pose with no upscaler and no jitter.
fn final_pose_aliased(path: &Path) {
    let mut renderer = headless(TAA_SIZE);
    renderer.set_material_time(1.25);
    renderer
        .set_post_settings(Some(PostSettings::default()))
        .unwrap();
    let m = meshes(&mut renderer);
    let x = 0.06 * 23.0;
    let eye = Vec3::new(x - 0.7, 2.6, 9.0);
    let camera = look_at(eye, eye + Vec3::new(-0.1, -1.8, -9.0), TAA_SIZE);
    renderer
        .render_frame(&lowpoly_world(lowpoly_instances(&m), camera))
        .unwrap();
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

#[test]
fn taa_with_a_moving_camera_matches_its_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("taa_moving.png");
    moving_camera(true, &out);
    let got = load_rgba(&out);
    let error = max_channel_error(&got, &load_rgba(&reference("taa_moving")));
    assert!(
        error <= 2,
        "max channel error {error} (output: {})",
        out.display()
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn a_moving_camera_leaves_no_ghosting() {
    let (messages, _guard) = capture_messages();
    let taa = out_path("ghost_taa.png");
    let aliased = out_path("ghost_aliased.png");
    moving_camera(true, &taa);
    final_pose_aliased(&aliased);
    let (a, b) = (load_rgba(&taa), load_rgba(&aliased));
    // The two differ by the anti-aliasing of the edges only: a trail would
    // smear whole objects.
    let mean = mean_error(&a, &b);
    assert!(mean < 3.0, "mean error {mean} against the aliased frame");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Wake pixels of a bright cube that crossed a dark scene: where the cube was
/// a few frames ago but is not now.
#[test]
fn a_moving_object_leaves_no_trail() {
    let (messages, _guard) = capture_messages();
    let eye = Vec3::new(0.0, 2.0, 8.0);
    let camera = look_at(eye, Vec3::new(0.0, 0.5, 0.0), TAA_SIZE);
    let frames = 14u32;
    let cube_x = |f: u32| -4.0 + 0.5 * f as f32;
    let cube_world = |f: u32| Mat4::from_translation(Vec3::new(cube_x(f), 0.5, 0.0));

    let run = |with_cube: bool, path: &Path| {
        let mut renderer = headless(TAA_SIZE);
        renderer.set_material_time(1.25);
        let (_, p) = pass(&renderer, UpscalerKind::Taa, Quality::Native);
        renderer.set_upscaler(Some(p)).unwrap();
        let m = meshes(&mut renderer);
        for f in 0..frames {
            let mut instances = vec![Instance::new(
                m.ground,
                0,
                Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
                    * Mat4::from_scale(Vec3::new(200.0, 1.0, 200.0)),
            )];
            if with_cube {
                instances.push(Instance {
                    mesh: m.cube,
                    material: 0,
                    world: cube_world(f),
                    prev_world: cube_world(f.saturating_sub(1)),
                });
            }
            let mut cam = camera;
            cam.jitter_px = renderer.upscaler_jitter(f);
            renderer
                .render_frame(&lowpoly_world(instances, cam))
                .unwrap();
        }
        renderer.read_back_png(path).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    };
    let (with, without) = (out_path("trail_with.png"), out_path("trail_without.png"));
    run(true, &with);
    run(false, &without);
    let (with, without) = (load_rgba(&with), load_rgba(&without));

    // The cube is at the right edge of the sweep now; look at where it was
    // 4 to 7 frames ago (2 to 3.5 m behind it, more than its width).
    let vp = camera.view_proj();
    let pixel_of = |p: Vec3| {
        let c = vp * p.extend(1.0);
        let ndc = c.truncate().truncate() / c.w;
        (
            ((ndc.x * 0.5 + 0.5) * TAA_SIZE.0 as f32) as i32,
            ((ndc.y * 0.5 + 0.5) * TAA_SIZE.1 as f32) as i32,
        )
    };
    let mut worst = 0u8;
    for back in 4..=7 {
        let (px, py) = pixel_of(Vec3::new(cube_x(frames - 1 - back), 0.5, 0.0));
        assert!(
            (3..TAA_SIZE.0 as i32 - 3).contains(&px) && (3..TAA_SIZE.1 as i32 - 3).contains(&py),
            "wake pixel ({px}, {py}) is off screen"
        );
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (x, y) = ((px + dx) as u32, (py + dy) as u32);
                let (a, b) = (with.get_pixel(x, y).0, without.get_pixel(x, y).0);
                worst = worst.max((0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap());
            }
        }
    }
    assert!(worst <= 6, "a trail of {worst}/255 where the cube was");
    // And the cube itself is there.
    let (cx, cy) = pixel_of(Vec3::new(cube_x(frames - 1), 0.5, 0.0));
    let (a, b) = (
        with.get_pixel(cx as u32, cy as u32).0,
        without.get_pixel(cx as u32, cy as u32).0,
    );
    assert!(
        (0..3).any(|i| a[i].abs_diff(b[i]) > 20),
        "the cube is not drawn: {a:?} vs {b:?}"
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// A frame with the reset flag is the current frame alone, whatever the
/// history held: here the camera teleports and the image must equal the
/// jittered scene without any accumulation.
#[test]
fn raising_the_reset_flag_discards_the_history() {
    let (messages, _guard) = capture_messages();
    let near = look_at(Vec3::new(0.0, 2.0, 8.0), Vec3::new(0.0, 0.5, 0.0), TAA_SIZE);
    let far = look_at(
        Vec3::new(9.0, 6.0, -8.0),
        Vec3::new(0.0, 0.5, 0.0),
        TAA_SIZE,
    );
    let jitter = faset_render::jitter(5);

    // Reference: the plain scene from the far pose with that jitter.
    let expected = out_path("reset_expected.png");
    {
        let mut renderer = headless(TAA_SIZE);
        renderer.set_material_time(1.25);
        let m = meshes(&mut renderer);
        let mut cam = far;
        cam.jitter_px = jitter;
        renderer
            .render_frame(&lowpoly_world(lowpoly_instances(&m), cam))
            .unwrap();
        renderer.read_back_png(&expected).unwrap();
    }
    let run = |reset: bool, path: &Path| {
        let mut renderer = headless(TAA_SIZE);
        renderer.set_material_time(1.25);
        let (_, p) = pass(&renderer, UpscalerKind::Taa, Quality::Native);
        renderer.set_upscaler(Some(p)).unwrap();
        let m = meshes(&mut renderer);
        let instances = lowpoly_instances(&m);
        for f in 0..6 {
            let mut cam = near;
            cam.jitter_px = renderer.upscaler_jitter(f);
            renderer
                .render_frame(&lowpoly_world(instances.clone(), cam))
                .unwrap();
        }
        // The teleport: no motion vector can follow it.
        if reset {
            renderer.reset_temporal_history();
        }
        let mut cam = far;
        cam.jitter_px = jitter;
        renderer
            .render_frame(&lowpoly_world(instances, cam))
            .unwrap();
        renderer.read_back_png(path).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    };
    let (with_reset, without_reset) = (out_path("reset_on.png"), out_path("reset_off.png"));
    run(true, &with_reset);
    run(false, &without_reset);
    let expected = load_rgba(&expected);
    assert!(
        max_channel_error(&load_rgba(&with_reset), &expected) <= 2,
        "the history leaked through the reset"
    );
    assert!(
        max_channel_error(&load_rgba(&without_reset), &expected) > 2,
        "without the flag the stale history should show"
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}
