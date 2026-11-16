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
    cube_mesh, jitter, perspective_reverse_z, Camera, Instance, MaterialTable, MeshId, Pattern,
    RenderConfig, RenderError, RenderWorld, Renderer, CUBE_FACE_TINTS,
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
            "out.color = vec4<f32>(in.color * material, 1.0);",
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
