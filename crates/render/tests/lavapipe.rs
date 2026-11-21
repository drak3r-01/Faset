//! Headless renderer tests on Mesa lavapipe (run by `cargo xtask render-test`).
//!
//! Every test asserts zero validation messages, counted twice: by the
//! renderer's debug messenger, and by a tracing layer that also sees messages
//! emitted while the device and instance are destroyed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_render::glam::{Mat4, Vec2, Vec3};
use faset_render::{
    cube_mesh, jitter, perspective_reverse_z, AmbienceProfile, Camera, Instance, MaterialTable,
    MeshId, Pattern, PointLight, RenderConfig, RenderError, RenderWorld, Renderer,
    BUILTIN_PROFILES, CUBE_FACE_TINTS, SHADOW_DISTANCE,
};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// Linear clear colour of the reference image.
const CLEAR: [f32; 4] = [0.25, 0.5, 0.75, 1.0];
const REFERENCE: &str = "tests/refs/clear_256.png";
const CUBES_REFERENCE: &str = "tests/refs/cubes.png";

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
    let dir = std::env::temp_dir().join("faset-render-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
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

fn srgb_encode(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// sRGB bytes the pipeline produces for a unorm vertex colour.
fn expected_rgb(rgb: [u8; 3]) -> [u8; 3] {
    rgb.map(|c| srgb_encode(f32::from(c) / 255.0))
}

fn assert_pixel(img: &image::RgbaImage, x: u32, y: u32, rgb: [u8; 3]) {
    let got = img.get_pixel(x, y).0;
    for (g, e) in got.iter().zip(rgb) {
        assert!(
            g.abs_diff(e) <= 1,
            "pixel ({x}, {y}) = {got:?}, expected {rgb:?}"
        );
    }
}

fn world_with(camera: Camera, instances: Vec<Instance>) -> RenderWorld {
    RenderWorld {
        clear_color: CLEAR,
        camera,
        instances,
        ..RenderWorld::default()
    }
}

#[test]
fn clear_256_matches_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("clear_256.png");
    {
        let mut renderer = headless((256, 256));
        let world = RenderWorld {
            clear_color: CLEAR,
            ..RenderWorld::default()
        };
        // Several frames so both frame slots are reused at least once.
        for _ in 0..4 {
            renderer.render_frame(&world).unwrap();
        }
        assert_eq!(renderer.frames_rendered(), 4);
        renderer.read_back_png(&out).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    let reference = load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join(REFERENCE));
    let got = load_rgba(&out);
    let error = max_channel_error(&got, &reference);
    assert!(error <= 1, "max channel error {error} > 1");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

#[test]
fn resize_to_zero_skips_frames_then_recovers() {
    let (messages, _guard) = capture_messages();
    let out = out_path("resized.png");
    {
        let mut renderer = headless((32, 32));
        let world = RenderWorld {
            clear_color: CLEAR,
            ..RenderWorld::default()
        };
        renderer.render_frame(&world).unwrap();
        assert_eq!(renderer.frames_rendered(), 1);

        renderer.resize(0, 0);
        renderer.resize(0, 40);
        for _ in 0..3 {
            renderer.render_frame(&world).unwrap();
        }
        assert_eq!(
            renderer.frames_rendered(),
            1,
            "zero-area frames must be skipped"
        );
        assert!(matches!(
            renderer.read_back_png(&out),
            Err(RenderError::NothingRendered)
        ));

        // A new size gets a fresh target; the old one goes through the
        // deferred queue, released two frames later.
        renderer.resize(48, 24);
        assert!(matches!(
            renderer.read_back_png(&out),
            Err(RenderError::NothingRendered)
        ));
        for _ in 0..4 {
            renderer.render_frame(&world).unwrap();
        }
        assert_eq!(renderer.frames_rendered(), 5);
        assert_eq!(renderer.size(), (48, 24));
        renderer.read_back_png(&out).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    let got = load_rgba(&out);
    assert_eq!(got.dimensions(), (48, 24));
    let expected = [
        srgb_encode(CLEAR[0]),
        srgb_encode(CLEAR[1]),
        srgb_encode(CLEAR[2]),
        255,
    ];
    for pixel in got.pixels() {
        for (g, e) in pixel.0.iter().zip(expected) {
            assert!(g.abs_diff(e) <= 1, "pixel {pixel:?} vs {expected:?}");
        }
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

#[test]
fn zero_sized_headless_renderer_is_valid() {
    let (messages, _guard) = capture_messages();
    {
        let mut renderer = headless((0, 0));
        renderer.render_frame(&RenderWorld::default()).unwrap();
        assert_eq!(renderer.frames_rendered(), 0);
        renderer.resize(16, 16);
        renderer.render_frame(&RenderWorld::default()).unwrap();
        assert_eq!(renderer.frames_rendered(), 1);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// The reference scene: a 5 x 5 grid of cubes, one tint per face direction,
/// each turned a little more around Y, seen from above at an angle.
fn render_cubes(path: &Path) {
    let mut renderer = headless((256, 256));
    let (v, i) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
    let cube = renderer.upload_mesh(&v, &i).unwrap();
    let mut instances = Vec::new();
    for z in 0..5 {
        for x in 0..5 {
            let k = (z * 5 + x) as f32;
            let pos = Vec3::new((x as f32 - 2.0) * 1.6, 0.0, (z as f32 - 2.0) * 1.6);
            let world = Mat4::from_translation(pos) * Mat4::from_rotation_y(0.12 * k);
            instances.push(Instance::new(cube, 0, world));
        }
    }
    let view = faset_render::glam::camera::rh::view::look_at_mat4(
        Vec3::new(4.0, 6.0, 8.0),
        Vec3::ZERO,
        Vec3::Y,
    );
    let proj = perspective_reverse_z(55f32.to_radians(), 1.0, 0.1);
    for frame in 0..4u32 {
        let mut camera = Camera::new(view, proj);
        camera.jitter_px = jitter(frame);
        renderer
            .render_frame(&world_with(camera, instances.clone()))
            .unwrap();
        let stats = renderer.last_frame_stats();
        assert_eq!(stats.instances_drawn, 25);
        assert_eq!(stats.draw_calls, 1);
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

#[test]
fn cubes_match_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("cubes.png");
    render_cubes(&out);
    let reference = load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join(CUBES_REFERENCE));
    let got = load_rgba(&out);
    let error = max_channel_error(&got, &reference);
    assert!(
        error <= 1,
        "max channel error {error} > 1 (output: {})",
        out.display()
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

#[test]
fn near_object_occludes_far_one_and_infinity_is_not_clipped() {
    let (messages, _guard) = capture_messages();
    let out = out_path("depth.png");
    let red = [200, 30, 30];
    let green = [30, 200, 30];
    let blue = [30, 30, 200];
    {
        let mut renderer = headless((64, 64));
        let mut upload = |rgb| {
            let (v, i) = cube_mesh(0.5, &[rgb; 6], 0);
            renderer.upload_mesh(&v, &i).unwrap()
        };
        let (near_mesh, far_mesh, distant_mesh) = (upload(red), upload(green), upload(blue));
        let camera = Camera::new(
            Mat4::IDENTITY,
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        // The far cube (front face at 4 m, fills the view) is listed first:
        // only the depth test can put the near one (2.5 m) in front.
        let far = Instance::new(
            far_mesh,
            0,
            Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0))
                * Mat4::from_scale(Vec3::splat(12.0)),
        );
        let near = Instance::new(
            near_mesh,
            0,
            Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)),
        );
        renderer
            .render_frame(&world_with(camera, vec![far, near]))
            .unwrap();
        renderer.read_back_png(&out).unwrap();
        let img = load_rgba(&out);
        assert_pixel(&img, 32, 32, expected_rgb(red));
        // Outside the near cube (|ndc| > 0.2) but inside the far one.
        assert_pixel(&img, 8, 32, expected_rgb(green));

        // 100 km away, 20 km wide: still drawn with the far plane at infinity.
        let distant = Instance::new(
            distant_mesh,
            0,
            Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0e5))
                * Mat4::from_scale(Vec3::splat(2.0e4)),
        );
        renderer
            .render_frame(&world_with(camera, vec![distant]))
            .unwrap();
        renderer.read_back_png(&out).unwrap();
        let img = load_rgba(&out);
        assert_pixel(&img, 32, 32, expected_rgb(blue));
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

/// A wall filling the view at 9.5 m (front face of a 40 x 40 x 1 box).
fn wall(renderer: &mut Renderer) -> Instance {
    let (v, i) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
    let mesh = renderer.upload_mesh(&v, &i).unwrap();
    Instance::new(
        mesh,
        0,
        Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0))
            * Mat4::from_scale(Vec3::new(40.0, 40.0, 1.0)),
    )
}

#[test]
fn static_scene_has_zero_motion() {
    let (messages, _guard) = capture_messages();
    {
        let mut renderer = headless((32, 32));
        let wall = wall(&mut renderer);
        let mut camera = Camera::new(
            faset_render::glam::camera::rh::view::look_at_mat4(
                Vec3::new(0.3, 0.2, 0.0),
                Vec3::new(0.0, 0.0, -10.0),
                Vec3::Y,
            ),
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        for frame in 0..3 {
            // Jitter changes every frame; motion vectors must ignore it.
            camera.jitter_px = jitter(frame);
            renderer
                .render_frame(&world_with(camera, vec![wall]))
                .unwrap();
            let motion = renderer.read_back_motion().unwrap();
            assert_eq!(motion.len(), 32 * 32);
            assert!(motion.iter().all(|m| *m == [0.0, 0.0]), "non-zero motion");
        }
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn translated_camera_gives_expected_motion() {
    let (messages, _guard) = capture_messages();
    {
        let mut renderer = headless((32, 32));
        let wall = wall(&mut renderer);
        let proj = perspective_reverse_z(90f32.to_radians(), 1.0, 0.1);
        // The camera moved 0.5 m right (+x) and 0.25 m up (+y) since the
        // previous frame.
        let d = Vec3::new(0.5, 0.25, 0.0);
        let camera = Camera {
            view: Mat4::from_translation(-d),
            proj,
            prev_view_proj: proj,
            jitter_px: jitter(7),
        };
        renderer
            .render_frame(&world_with(camera, vec![wall]))
            .unwrap();
        let motion = renderer.read_back_motion().unwrap();
        // The wall's front face is 9.5 m away and f / a = 1. The scene moves
        // left on screen (negative x) and down (positive y: Vulkan NDC).
        let depth = 9.5;
        let expected = Vec2::new(-0.5 * d.x / depth, 0.5 * d.y / depth);
        for m in &motion {
            let m = Vec2::from_array(*m);
            assert!(
                (m - expected).abs().max_element() < 1e-4,
                "motion {m} vs {expected}"
            );
        }
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn culling_draws_only_visible_instances() {
    let (messages, _guard) = capture_messages();
    {
        let mut renderer = headless((64, 64));
        let (v, i) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
        let meshes: Vec<MeshId> = (0..20)
            .map(|_| renderer.upload_mesh(&v, &i).unwrap())
            .collect();
        let camera = Camera::new(
            Mat4::IDENTITY,
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        let mut instances = Vec::new();
        for k in 0..10_000u32 {
            let n = k / 10;
            let mesh = meshes[(n % 20) as usize];
            let pos = if k % 10 == 0 {
                // 1 000 visible, in a 40 x 25 grid at 60 m.
                Vec3::new((n % 40) as f32 - 20.0, (n / 40) as f32 - 12.0, -60.0)
            } else {
                // 9 000 behind the camera.
                Vec3::new((k % 50) as f32, (k % 7) as f32, 5.0 + (k % 30) as f32)
            };
            instances.push(Instance::new(mesh, 0, Mat4::from_translation(pos)));
        }
        renderer
            .render_frame(&world_with(camera, instances))
            .unwrap();
        let stats = renderer.last_frame_stats();
        assert_eq!(stats.instances_drawn, 1000);
        assert_eq!(stats.instances_culled, 9000);
        assert_eq!(stats.draw_calls, 20, "one draw per visible mesh");
        assert!(stats.draw_calls < 500);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn invalid_meshes_are_rejected() {
    let (messages, _guard) = capture_messages();
    {
        let mut renderer = headless((8, 8));
        let (v, _) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
        assert!(matches!(
            renderer.upload_mesh(&v, &[0, 1]),
            Err(RenderError::InvalidMesh)
        ));
        assert!(matches!(
            renderer.upload_mesh(&v, &[0, 1, 24]),
            Err(RenderError::InvalidMesh)
        ));
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Writes the reference image from the sRGB transfer function, independently
/// of the GPU. Run once: `cargo test -p faset-render --test lavapipe -- --ignored`.
#[test]
#[ignore = "regenerates tests/refs/clear_256.png"]
fn bless_clear_reference() {
    let pixel = image::Rgba([
        srgb_encode(CLEAR[0]),
        srgb_encode(CLEAR[1]),
        srgb_encode(CLEAR[2]),
        srgb_encode(CLEAR[3]),
    ]);
    let img = image::RgbaImage::from_pixel(256, 256, pixel);
    img.save(Path::new(env!("CARGO_MANIFEST_DIR")).join(REFERENCE))
        .unwrap();
}

/// Regenerates the cube reference from a lavapipe render. Inspect the image
/// before committing it.
#[test]
#[ignore = "regenerates tests/refs/cubes.png"]
fn bless_cubes_reference() {
    render_cubes(&Path::new(env!("CARGO_MANIFEST_DIR")).join(CUBES_REFERENCE));
}

const SWATCH_REFERENCE: &str = "tests/refs/pattern_swatch.png";
/// Fixed material time of the swatch, in seconds.
const SWATCH_TIME: f32 = 1.25;

/// One white cube per pattern (built-in material ids 0..=13), in a 4 x 4
/// grid seen from the front and slightly above, each turned to show two
/// faces plus its top.
fn render_swatch(path: &Path) {
    let mut renderer = headless((256, 256));
    renderer.set_material_time(SWATCH_TIME);
    let mut instances = Vec::new();
    for (k, pattern) in Pattern::ALL.iter().enumerate() {
        let (v, i) = cube_mesh(0.8, &[[255; 3]; 6], *pattern as u8);
        let mesh = renderer.upload_mesh(&v, &i).unwrap();
        let (col, row) = ((k % 4) as f32, (k / 4) as f32);
        let pos = Vec3::new((col - 1.5) * 2.2, (1.5 - row) * 2.2 + 4.0, 0.0);
        let world = Mat4::from_translation(pos) * Mat4::from_rotation_y(0.6);
        instances.push(Instance::new(mesh, 0, world));
    }
    let view = faset_render::glam::camera::rh::view::look_at_mat4(
        Vec3::new(0.0, 7.0, 11.0),
        Vec3::new(0.0, 4.0, 0.0),
        Vec3::Y,
    );
    let proj = perspective_reverse_z(50f32.to_radians(), 1.0, 0.1);
    renderer
        .render_frame(&world_with(Camera::new(view, proj), instances))
        .unwrap();
    assert_eq!(renderer.last_frame_stats().instances_drawn, 14);
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

#[test]
fn pattern_swatch_matches_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("pattern_swatch.png");
    render_swatch(&out);
    let reference = load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join(SWATCH_REFERENCE));
    let got = load_rgba(&out);
    let error = max_channel_error(&got, &reference);
    assert!(
        error <= 1,
        "max channel error {error} > 1 (output: {})",
        out.display()
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

/// Regenerates the swatch reference from a lavapipe render. Inspect the
/// image before committing it.
#[test]
#[ignore = "regenerates tests/refs/pattern_swatch.png"]
fn bless_pattern_swatch_reference() {
    render_swatch(&Path::new(env!("CARGO_MANIFEST_DIR")).join(SWATCH_REFERENCE));
}

/// Fixed material time of the profile scene, in seconds.
const PROFILE_TIME: f32 = 1.25;

fn profile_reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/profile_{name}.png"))
}

/// The ambience reference scene: a sand ground, rows of grey cubes receding
/// into the fog, a brick block, lava, emissive and hologram props and four
/// coloured point lights, lit by the built-in profile `name`.
fn render_profile(name: &str, path: &Path) {
    let ambience = AmbienceProfile::builtin(name).expect("built-in profile");
    let mut renderer = headless((256, 256));
    renderer.set_material_time(PROFILE_TIME);
    let mut mesh = |rgb: [u8; 3], material: u8| {
        let (v, i) = cube_mesh(0.5, &[rgb; 6], material);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    let (ground, grey, brick) = (mesh([255; 3], 2), mesh([210; 3], 0), mesh([255; 3], 6));
    let (lava, emissive, hologram) = (mesh([255; 3], 11), mesh([255; 3], 12), mesh([255; 3], 13));
    let place = |mesh, pos: Vec3, scale: Vec3, turn: f32| {
        Instance::new(
            mesh,
            0,
            Mat4::from_translation(pos) * Mat4::from_rotation_y(turn) * Mat4::from_scale(scale),
        )
    };
    let mut instances = vec![
        place(
            ground,
            Vec3::new(0.0, -0.5, -20.0),
            Vec3::new(80.0, 1.0, 80.0),
            0.0,
        ),
        place(brick, Vec3::new(-3.5, 1.0, 0.0), Vec3::splat(2.0), 0.3),
        place(lava, Vec3::new(3.5, 0.5, 0.0), Vec3::ONE, 0.5),
        place(
            emissive,
            Vec3::new(0.0, 1.5, -6.0),
            Vec3::new(0.6, 3.0, 0.6),
            0.0,
        ),
        place(hologram, Vec3::new(-1.5, 0.5, 1.5), Vec3::ONE, 0.8),
    ];
    for row in 0..5 {
        for col in -2..=2 {
            let pos = Vec3::new(col as f32 * 3.0, 0.5, 4.0 - row as f32 * 8.0);
            instances.push(place(grey, pos, Vec3::ONE, 0.4 + 0.2 * col as f32));
        }
    }
    let light = |position: Vec3, color: Vec3, radius| PointLight {
        position,
        color,
        radius,
    };
    let lights = vec![
        light(Vec3::new(-1.5, 1.2, 0.5), Vec3::new(3.0, 1.2, 0.3), 5.0),
        light(Vec3::new(1.5, 1.0, -2.0), Vec3::new(0.3, 2.0, 3.0), 5.0),
        light(Vec3::new(0.0, 2.0, -9.0), Vec3::new(3.0, 0.3, 2.5), 7.0),
        light(Vec3::new(-4.0, 1.0, -14.0), Vec3::new(0.5, 3.0, 0.8), 6.0),
    ];
    let view = faset_render::glam::camera::rh::view::look_at_mat4(
        Vec3::new(0.0, 4.5, 12.0),
        Vec3::new(0.0, 0.5, -6.0),
        Vec3::Y,
    );
    let proj = perspective_reverse_z(60f32.to_radians(), 1.0, 0.1);
    let fog = ambience.fog.color;
    let world = RenderWorld {
        // The horizon blends into the fog (the sky arrives in lot 2.6).
        clear_color: [fog[0], fog[1], fog[2], 1.0],
        camera: Camera::new(view, proj),
        instances,
        lights,
        ambience,
    };
    renderer.render_frame(&world).unwrap();
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

#[test]
fn ambience_profiles_match_references() {
    let (messages, _guard) = capture_messages();
    let mut failures = Vec::new();
    for name in BUILTIN_PROFILES {
        let out = out_path(&format!("profile_{name}.png"));
        render_profile(name, &out);
        let error = max_channel_error(&load_rgba(&out), &load_rgba(&profile_reference(name)));
        if error > 1 {
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

/// Regenerates the nine profile references from lavapipe renders. Inspect
/// the images before committing them.
#[test]
#[ignore = "regenerates tests/refs/profile_*.png"]
fn bless_profile_references() {
    for name in BUILTIN_PROFILES {
        render_profile(name, &profile_reference(name));
    }
}

/// Point lights light what is inside their radius and nothing beyond, and
/// more than 32 lights render without validation errors.
#[test]
fn point_lights_stop_at_their_radius() {
    let (messages, _guard) = capture_messages();
    let out = out_path("point_lights.png");
    {
        let mut renderer = headless((32, 32));
        let wall = wall(&mut renderer);
        let camera = Camera::new(
            Mat4::IDENTITY,
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        // Black ambient: only the lights show.
        let mut ambience = AmbienceProfile::default();
        ambience.ambient.ground = [0.0; 3];
        ambience.ambient.sky = [0.0; 3];
        let mut world = world_with(camera, vec![wall]);
        world.ambience = ambience;
        let centre = |renderer: &mut Renderer, world: &RenderWorld| {
            renderer.render_frame(world).unwrap();
            renderer.read_back_png(&out).unwrap();
            load_rgba(&out).get_pixel(16, 16).0
        };
        // The wall's front face is 9.5 m away; a light 1 m in front of it
        // lights its centre.
        let near = PointLight {
            position: Vec3::new(0.0, 0.0, -8.5),
            color: Vec3::ONE,
            radius: 3.0,
        };
        world.lights = vec![near];
        let lit = centre(&mut renderer, &world);
        let brightness: u32 = lit[..3].iter().map(|c| u32::from(*c)).sum();
        assert!(brightness > 150, "lit pixel {lit:?}");
        // The same light with a radius equal to the distance: black.
        world.lights = vec![PointLight {
            radius: 1.0,
            ..near
        }];
        assert_eq!(&centre(&mut renderer, &world)[..3], &[0, 0, 0]);
        // 40 lights, all out of range: dropped beyond 32, still black.
        world.lights = vec![
            PointLight {
                radius: 0.5,
                ..near
            };
            40
        ];
        assert_eq!(&centre(&mut renderer, &world)[..3], &[0, 0, 0]);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

fn srgb_decode(c: u8) -> f32 {
    let s = f32::from(c) / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Patterned materials seen on a wall 1 km away: the pattern must settle to
/// a flat tone (every pixel's linear luminance within 5 % of the mean)
/// instead of aliasing. Flat and gradient have no detail to alias.
#[test]
fn distant_patterns_do_not_shimmer() {
    let (messages, _guard) = capture_messages();
    let out = out_path("far.png");
    {
        let mut renderer = headless((128, 128));
        renderer.set_material_time(SWATCH_TIME);
        let camera = Camera::new(
            faset_render::glam::camera::rh::view::look_at_mat4(
                Vec3::new(3.7, 1.8, 0.0),
                Vec3::new(3.7, 1.8, -1.0),
                Vec3::Y,
            ),
            perspective_reverse_z(60f32.to_radians(), 1.0, 0.1),
        );
        for pattern in Pattern::ALL {
            if matches!(pattern, Pattern::Flat | Pattern::Gradient) {
                continue;
            }
            let (v, i) = cube_mesh(0.5, &[[255; 3]; 6], pattern as u8);
            let mesh = renderer.upload_mesh(&v, &i).unwrap();
            // Front face at 1 000 m, 4 km wide: fills the view.
            let wall = Instance::new(
                mesh,
                0,
                Mat4::from_translation(Vec3::new(0.0, 0.0, -1000.5))
                    * Mat4::from_scale(Vec3::new(4000.0, 4000.0, 1.0)),
            );
            renderer
                .render_frame(&world_with(camera, vec![wall]))
                .unwrap();
            renderer.read_back_png(&out).unwrap();
            let img = load_rgba(&out);
            let luminance: Vec<f32> = img
                .pixels()
                .map(|p| {
                    let [r, g, b, _] = p.0;
                    0.2126 * srgb_decode(r) + 0.7152 * srgb_decode(g) + 0.0722 * srgb_decode(b)
                })
                .collect();
            let mean = luminance.iter().sum::<f32>() / luminance.len() as f32;
            let worst = luminance
                .iter()
                .map(|l| (l - mean).abs())
                .fold(0.0, f32::max);
            assert!(mean > 0.0, "{pattern:?}: black wall");
            assert!(
                worst <= 0.05 * mean,
                "{pattern:?}: luminance deviates {:.1} % from the mean {mean:.4}",
                100.0 * worst / mean
            );
        }
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

#[test]
fn custom_material_table_is_used() {
    let (messages, _guard) = capture_messages();
    let out = out_path("custom_material.png");
    {
        let mut renderer = headless((16, 16));
        let table = MaterialTable::from_ron(
            "[(id: 200, pattern: Flat, color_a: (0.5, 0.25, 1.0), color_b: (0.0, 0.0, 0.0), scale: 1.0)]",
        )
        .unwrap();
        renderer.set_materials(&table).unwrap();
        let (v, i) = cube_mesh(0.5, &[[255, 255, 128]; 6], 200);
        let mesh = renderer.upload_mesh(&v, &i).unwrap();
        let camera = Camera::new(
            Mat4::IDENTITY,
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        let wall = Instance::new(
            mesh,
            0,
            Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0))
                * Mat4::from_scale(Vec3::new(40.0, 40.0, 1.0)),
        );
        renderer
            .render_frame(&world_with(camera, vec![wall]))
            .unwrap();
        renderer.read_back_png(&out).unwrap();
        let img = load_rgba(&out);
        // Vertex colour times material colour A.
        let blue = srgb_encode(128.0 / 255.0);
        assert_pixel(&img, 8, 8, [srgb_encode(0.5), srgb_encode(0.25), blue]);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

/// Counts `tracing::error!` events of the hot reload module.
#[cfg(feature = "hot-reload")]
struct ReloadErrors(Arc<AtomicUsize>);

#[cfg(feature = "hot-reload")]
impl<S: tracing::Subscriber> Layer<S> for ReloadErrors {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() == tracing::Level::ERROR && meta.target().contains("hot_reload") {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[cfg(feature = "hot-reload")]
fn assert_pixel_eq(got: [u8; 4], rgb: [u8; 3]) {
    for (g, e) in got.iter().zip(rgb) {
        assert!(g.abs_diff(e) <= 1, "pixel {got:?}, expected {rgb:?}");
    }
}

/// Edits a copy of the shader sources: a valid edit goes live within 60
/// frames; a broken edit keeps the previous pipeline and logs an error.
#[cfg(feature = "hot-reload")]
#[test]
fn hot_reload_swaps_valid_shaders_and_keeps_old_on_error() {
    use faset_render::ShaderWatcher;

    let messages = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(VulkanMessages(messages.clone()))
        .with(ReloadErrors(errors.clone()))
        .with(tracing_subscriber::fmt::layer().with_test_writer());
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = std::env::temp_dir().join(format!("faset-shader-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(faset_shaders::SOURCE_DIR).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "wgsl") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    let opaque = dir.join("opaque.wgsl");
    let original = std::fs::read_to_string(&opaque).unwrap();
    let out = out_path("hot_reload.png");
    {
        let mut renderer = headless((16, 16));
        let mut watcher = ShaderWatcher::new(&dir);
        let grey = [128, 128, 128];
        let (v, i) = cube_mesh(0.5, &[grey; 6], 0);
        let mesh = renderer.upload_mesh(&v, &i).unwrap();
        let camera = Camera::new(
            Mat4::IDENTITY,
            perspective_reverse_z(90f32.to_radians(), 1.0, 0.1),
        );
        let world = world_with(
            camera,
            vec![Instance::new(
                mesh,
                0,
                Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0))
                    * Mat4::from_scale(Vec3::new(40.0, 40.0, 1.0)),
            )],
        );
        let centre = |renderer: &mut Renderer| {
            renderer.render_frame(&world).unwrap();
            renderer.read_back_png(&out).unwrap();
            load_rgba(&out).get_pixel(8, 8).0
        };
        assert!(!watcher.poll(&mut renderer), "no edit yet");
        assert_pixel_eq(centre(&mut renderer), expected_rgb(grey));

        // Valid edit: paint everything red. One frame per poll.
        let red = original.replace(
            "out.color = vec4<f32>(apply_fog(lit, in.world_pos), 1.0);",
            "out.color = vec4<f32>(1.0, 0.0, 0.0, 1.0);",
        );
        assert_ne!(red, original, "edit target found");
        std::fs::write(&opaque, &red).unwrap();
        let mut reloaded = false;
        for _ in 0..60 {
            reloaded = watcher.poll(&mut renderer);
            renderer.render_frame(&world).unwrap();
            if reloaded {
                break;
            }
        }
        assert!(reloaded, "new pipeline not active within 60 frames");
        assert_pixel_eq(centre(&mut renderer), [255, 0, 0]);

        // Broken edit: the red pipeline stays and one error is logged.
        std::fs::write(&opaque, format!("{red}\nfn broken( {{\n")).unwrap();
        for _ in 0..3 {
            assert!(!watcher.poll(&mut renderer));
            assert_pixel_eq(centre(&mut renderer), [255, 0, 0]);
        }
        assert_eq!(
            errors.load(Ordering::SeqCst),
            1,
            "one error per failed edit"
        );
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(out);
}

const SHADOW_REFERENCE: &str = "tests/refs/shadow_cascade.png";

/// The shadow test profile: the built-in day profile without fog, with the
/// sun behind the camera, to its left and high, so shadows fall into view.
fn shadow_ambience() -> AmbienceProfile {
    let mut ambience = AmbienceProfile::builtin("day").expect("built-in profile");
    ambience.sun.direction = [-0.55, 0.6, 0.5];
    ambience.fog.density = 0.0;
    ambience
}

fn shadow_view(eye: Vec3) -> Camera {
    Camera::new(
        faset_render::glam::camera::rh::view::look_at_mat4(
            eye,
            eye + Vec3::new(0.0, -5.0, -18.0),
            Vec3::Y,
        ),
        perspective_reverse_z(60f32.to_radians(), 1.0, 0.1),
    )
}

const SHADOW_EYE: Vec3 = Vec3::new(0.0, 5.0, 10.0);

/// A sand ground and boxes at 8 to 45 m from the camera, so that all three
/// cascades hold shadow edges.
fn shadow_scene(renderer: &mut Renderer) -> Vec<Instance> {
    let mut mesh = |rgb: [u8; 3]| {
        let (v, i) = cube_mesh(0.5, &[rgb; 6], 0);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    let (sand, red, blue, grey) = (
        mesh([200, 180, 140]),
        mesh([200, 70, 60]),
        mesh([70, 100, 200]),
        mesh([190, 190, 190]),
    );
    let place = |mesh, pos: Vec3, scale: Vec3, turn: f32| {
        Instance::new(
            mesh,
            0,
            Mat4::from_translation(pos) * Mat4::from_rotation_y(turn) * Mat4::from_scale(scale),
        )
    };
    vec![
        place(
            sand,
            Vec3::new(0.0, -0.5, -20.0),
            Vec3::new(120.0, 1.0, 120.0),
            0.0,
        ),
        place(red, Vec3::new(-2.5, 0.75, 2.0), Vec3::splat(1.5), 0.3),
        place(
            blue,
            Vec3::new(2.0, 2.0, -10.0),
            Vec3::new(1.5, 4.0, 1.5),
            0.0,
        ),
        place(
            grey,
            Vec3::new(-3.0, 1.0, -4.0),
            Vec3::new(1.0, 2.0, 1.0),
            0.7,
        ),
        place(
            red,
            Vec3::new(4.0, 1.5, -20.0),
            Vec3::new(2.0, 3.0, 2.0),
            0.2,
        ),
        place(
            grey,
            Vec3::new(-6.0, 3.0, -36.0),
            Vec3::new(10.0, 6.0, 1.5),
            0.5,
        ),
        place(
            blue,
            Vec3::new(8.0, 4.0, -42.0),
            Vec3::new(3.0, 8.0, 3.0),
            0.0,
        ),
    ]
}

fn shadow_world(instances: Vec<Instance>) -> RenderWorld {
    RenderWorld {
        clear_color: [0.55, 0.7, 0.9, 1.0],
        camera: shadow_view(SHADOW_EYE),
        instances,
        ambience: shadow_ambience(),
        ..RenderWorld::default()
    }
}

fn render_shadow_reference(path: &Path) {
    let mut renderer = headless((256, 256));
    let instances = shadow_scene(&mut renderer);
    let world = shadow_world(instances);
    // Twice, so both frame slots (and the shared shadow maps) are reused.
    for _ in 0..2 {
        renderer.render_frame(&world).unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

#[test]
fn shadow_cascade_matches_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("shadow_cascade.png");
    render_shadow_reference(&out);
    let reference = load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join(SHADOW_REFERENCE));
    let got = load_rgba(&out);
    let error = max_channel_error(&got, &reference);
    assert!(
        error <= 1,
        "max channel error {error} > 1 (output: {})",
        out.display()
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

/// Regenerates the shadow reference from a lavapipe render. Inspect the
/// image before committing it.
#[test]
#[ignore = "regenerates tests/refs/shadow_cascade.png"]
fn bless_shadow_reference() {
    render_shadow_reference(&Path::new(env!("CARGO_MANIFEST_DIR")).join(SHADOW_REFERENCE));
}

/// Pixel of a world point seen by `camera` in a `size` x `size` image.
fn pixel_of(camera: &Camera, p: Vec3, size: u32) -> (u32, u32) {
    let clip = camera.view_proj() * p.extend(1.0);
    let ndc = clip.truncate().truncate() / clip.w;
    let to_px = |v: f32| ((v * 0.5 + 0.5) * size as f32) as u32;
    (to_px(ndc.x), to_px(ndc.y))
}

fn luminance(img: &image::RgbaImage, (x, y): (u32, u32)) -> f32 {
    let [r, g, b, _] = img.get_pixel(x, y).0;
    0.2126 * srgb_decode(r) + 0.7152 * srgb_decode(g) + 0.0722 * srgb_decode(b)
}

/// A cast shadow darkens the ground behind its box and nothing else does;
/// with the sun off the same point is lit.
#[test]
fn box_casts_a_shadow_on_the_ground() {
    let (messages, _guard) = capture_messages();
    let out = out_path("shadow_ground.png");
    {
        let mut renderer = headless((256, 256));
        let instances = shadow_scene(&mut renderer);
        let mut world = shadow_world(instances);
        // The 4 m pillar at (2, 0..4, -10): its top centre, projected along
        // the sun onto the ground, is in shadow.
        let sun = Vec3::from_array(world.ambience.sun.direction).normalize();
        let top = Vec3::new(2.0, 4.0, -10.0);
        let shadowed = top - sun * (top.y / sun.y);
        let lit = Vec3::new(-8.0, 0.0, -14.0);
        let sample = |renderer: &mut Renderer, world: &RenderWorld| {
            renderer.render_frame(world).unwrap();
            renderer.read_back_png(&out).unwrap();
            let img = load_rgba(&out);
            let at = |p| luminance(&img, pixel_of(&world.camera, p, 256));
            (at(shadowed), at(lit))
        };
        let (in_shadow, in_light) = sample(&mut renderer, &world);
        assert!(
            in_shadow < 0.6 * in_light,
            "shadowed {in_shadow:.4} vs lit {in_light:.4}"
        );
        world.ambience.sun.intensity = 0.0;
        let (off_a, off_b) = sample(&mut renderer, &world);
        assert!(
            (off_a - off_b).abs() < 0.01 * off_a,
            "no sun, no shadow: {off_a:.4} vs {off_b:.4}"
        );
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}

/// The cascades follow a camera that moves in half-texel steps while the
/// view stays put: whenever the snapped matrices move, they move by whole
/// texels, so the maps are rasterised on the same world grid and no shadow
/// edge pixel changes between frames.
#[test]
fn shadow_edges_do_not_shimmer_while_the_cascades_move() {
    let (messages, _guard) = capture_messages();
    let out = out_path("shadow_stability.png");
    {
        let mut renderer = headless((256, 256));
        let instances = shadow_scene(&mut renderer);
        let world = shadow_world(instances);
        let sun = Vec3::from_array(world.ambience.sun.direction).normalize();
        let fit = |camera: &Camera| faset_render::fit_cascades(camera, sun, 0.1, SHADOW_DISTANCE);
        let texel = fit(&world.camera).texel_size(0);
        let direction = Vec3::new(1.0, 0.0, 0.6).normalize();

        let mut previous: Option<image::RgbaImage> = None;
        let mut previous_matrix = None;
        let (mut shifted, mut kept) = (0, 0);
        for step in 0..24 {
            let eye = SHADOW_EYE + direction * (0.5 * texel * step as f32);
            let shadow_camera = shadow_view(eye);
            renderer.set_shadow_camera(Some(shadow_camera));
            renderer.render_frame(&world).unwrap();
            renderer.read_back_png(&out).unwrap();
            let img = load_rgba(&out);
            let matrix = fit(&shadow_camera).matrices[0];
            if let (Some(prev), Some(prev_matrix)) = (&previous, previous_matrix) {
                if matrix == prev_matrix {
                    kept += 1;
                } else {
                    shifted += 1;
                }
                let changed = img
                    .pixels()
                    .zip(prev.pixels())
                    .filter(|(a, b)| a != b)
                    .count();
                assert_eq!(changed, 0, "step {step}: {changed} pixels changed");
            }
            previous = Some(img);
            previous_matrix = Some(matrix);
        }
        assert!(shifted >= 4 && kept >= 4, "shifted {shifted}, kept {kept}");
        // The frozen image does contain shadow edges.
        let img = previous.unwrap();
        let darkest = img.pixels().map(|p| p.0[0]).min().unwrap();
        let brightest = img.pixels().map(|p| p.0[0]).max().unwrap();
        assert!(brightest > darkest + 40);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
    let _ = std::fs::remove_file(out);
}
