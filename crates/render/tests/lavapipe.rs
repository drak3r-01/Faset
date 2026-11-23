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
    cube_mesh, jitter, perspective_reverse_z, AmbienceProfile, Camera, DeviceExtensions,
    GerstnerWave, Instance, MaterialTable, MeshId, Pattern, PointLight, PostParams, PostSettings,
    RenderConfig, RenderError, RenderWorld, Renderer, ScreenEffects, ShelterMap, SkyParams,
    SunSource, WaterParams, WeatherState, BUILTIN_PROFILES, CUBE_FACE_TINTS, SHADOW_DISTANCE,
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

/// The optional NVX and push-descriptor extensions are enabled exactly when
/// the device offers them (lavapipe offers some, an NVIDIA GPU all three),
/// and a device without them is created all the same.
#[test]
fn optional_extensions_are_enabled_when_offered() {
    let (messages, _guard) = capture_messages();
    {
        let renderer = headless((16, 16));
        let handles = renderer.gpu_handles();
        assert_eq!(handles.extensions, renderer.device_extensions());

        // SAFETY: the physical device belongs to the instance.
        let offered = unsafe {
            handles
                .instance
                .enumerate_device_extension_properties(handles.physical)
        }
        .unwrap();
        let offered = DeviceExtensions::from_offered(
            offered
                .iter()
                .filter_map(|e| e.extension_name_as_c_str().ok()),
        );
        assert_eq!(renderer.device_extensions(), offered);

        // The check the upscaler does: the command of an extension only
        // resolves on a device created with it.
        let commands = [
            (c"vkCreateCuModuleNVX", offered.nvx_binary_import),
            (c"vkGetImageViewHandleNVX", offered.nvx_image_view_handle),
            (c"vkCmdPushDescriptorSetKHR", offered.khr_push_descriptor),
        ];
        for (command, enabled) in commands {
            // SAFETY: a live device and a null-terminated command name.
            let found = unsafe {
                handles
                    .instance
                    .get_device_proc_addr(handles.device.handle(), command.as_ptr())
            };
            assert_eq!(found.is_some(), enabled, "{command:?}");
        }
        assert_eq!(
            renderer.device_extensions().dlss_ready(),
            renderer.device_extensions().missing().is_empty()
        );
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
    render_patterns(&Pattern::ALL[..14], path);
}

const PIXEL_SWATCH_REFERENCE: &str = "tests/refs/pixel_swatch.png";

/// The pixel-art patterns (ids 14..=27) in the same layout as the swatch.
fn render_pixel_swatch(path: &Path) {
    render_patterns(&Pattern::ALL[14..], path);
}

fn render_patterns(patterns: &[Pattern], path: &Path) {
    let mut renderer = headless((256, 256));
    renderer.set_material_time(SWATCH_TIME);
    let mut instances = Vec::new();
    for (k, pattern) in patterns.iter().enumerate() {
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

#[test]
fn pixel_swatch_matches_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("pixel_swatch.png");
    render_pixel_swatch(&out);
    let reference = load_rgba(&Path::new(env!("CARGO_MANIFEST_DIR")).join(PIXEL_SWATCH_REFERENCE));
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

/// Regenerates the pixel swatch reference. Inspect the image first.
#[test]
#[ignore = "regenerates tests/refs/pixel_swatch.png"]
fn bless_pixel_swatch_reference() {
    render_pixel_swatch(&Path::new(env!("CARGO_MANIFEST_DIR")).join(PIXEL_SWATCH_REFERENCE));
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

/// Water of the underwater scenes: the surface is 8 m above the ground.
fn scene_water() -> WaterParams {
    WaterParams {
        height: 8.0,
        waves: vec![
            GerstnerWave {
                amplitude: 0.3,
                steepness: 0.5,
                direction: Vec2::new(1.0, 0.2),
                wavelength: 12.0,
            },
            GerstnerWave {
                amplitude: 0.12,
                steepness: 0.6,
                direction: Vec2::new(-0.4, 1.0),
                wavelength: 6.0,
            },
        ],
    }
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
        // Without a sky the horizon blends into the fog; the space profile
        // draws its sky: nebula, planets and stars.
        clear_color: [fog[0], fog[1], fog[2], 1.0],
        camera: Camera::new(view, proj),
        instances,
        lights,
        ambience,
        weather: WeatherState::CLEAR,
        sky: SkyParams::default(),
        draw_sky: name == "space",
        // The underwater profile sits below a wavy surface, so it gets the
        // water medium: fog, caustics and suspended particles.
        water: (name == "underwater").then(scene_water),
        underwater: name == "underwater",
        screen: ScreenEffects::default(),
        post: None,
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
            "out.color = vec4<f32>(apply_fog(mix(lit, wet.reflection, wet.weight), in.world_pos), 1.0);",
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
            (off_a - off_b).abs() < 0.03 * off_a,
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

// ---------------------------------------------------------------------------
// Procedural sky (lot 2.6)
// ---------------------------------------------------------------------------

/// Name, hour, camera azimuth and pitch (degrees; azimuth from -Z towards
/// +X) of the four time-of-day references.
type SkyShot = (&'static str, f32, f32, f32);

const SKY_SHOTS: [SkyShot; 4] = [
    ("sky_06", 6.0, 90.0, 12.0),
    ("sky_12", 12.0, 180.0, 20.0),
    ("sky_18", 18.0, 270.0, 12.0),
    ("sky_00", 0.0, 180.0, 30.0),
];

fn sky_reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

/// The "day" profile driven by the map time, with stars for the night.
fn tod_ambience() -> AmbienceProfile {
    let mut a = AmbienceProfile::builtin("day").expect("built-in profile");
    a.sun.source = SunSource::TimeOfDay;
    a.sky.stars = 1.0;
    a
}

/// Camera at eye height looking along compass azimuth `az` (from -Z towards
/// +X) and `pitch`, both in degrees; also returns the horizontal forward
/// vector.
fn sky_camera(az: f32, pitch: f32) -> (Camera, Vec3) {
    let (az, pitch) = (az.to_radians(), pitch.to_radians());
    let dir = Vec3::new(az.sin() * pitch.cos(), pitch.sin(), -az.cos() * pitch.cos());
    let eye = Vec3::new(0.0, 1.6, 0.0);
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
    let proj = perspective_reverse_z(70f32.to_radians(), 1.0, 0.1);
    (Camera::new(view, proj), Vec3::new(az.sin(), 0.0, -az.cos()))
}

/// A sand ground and three cubes below a sky of the given hour.
fn render_sky(time_h: f32, az: f32, pitch: f32, cover: f32, path: &Path) {
    let mut renderer = headless((256, 256));
    renderer.set_material_time(PROFILE_TIME);
    let mut mesh = |material: u8| {
        let (v, i) = cube_mesh(0.5, &[[230; 3]; 6], material);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    let (ground, cube) = (mesh(2), mesh(0));
    let (camera, forward) = sky_camera(az, pitch);
    let right = forward.cross(Vec3::Y);
    let at = |ahead: f32, side: f32| Vec3::new(0.0, 0.5, 0.0) + forward * ahead + right * side;
    let mut instances = vec![Instance::new(
        ground,
        0,
        Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
            * Mat4::from_scale(Vec3::new(400.0, 1.0, 400.0)),
    )];
    for (ahead, side) in [(9.0, -2.5), (12.0, 1.5), (30.0, 6.0)] {
        instances.push(Instance::new(
            cube,
            0,
            Mat4::from_translation(at(ahead, side)),
        ));
    }
    let world = RenderWorld {
        camera,
        instances,
        ambience: tod_ambience(),
        sky: SkyParams {
            time_h,
            latitude_deg: 45.0,
            cloud_cover: cover,
            ..SkyParams::default()
        },
        draw_sky: true,
        ..RenderWorld::default()
    };
    // Twice, so both frame slots are used.
    for _ in 0..2 {
        renderer.render_frame(&world).unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
}

fn render_sky_shot(shot: SkyShot, path: &Path) {
    let (_, time, az, pitch) = shot;
    render_sky(time, az, pitch, 0.45, path);
}

#[test]
fn time_of_day_skies_match_references() {
    let (messages, _guard) = capture_messages();
    let mut failures = Vec::new();
    for shot in SKY_SHOTS {
        let out = out_path(&format!("{}.png", shot.0));
        render_sky_shot(shot, &out);
        let error = max_channel_error(&load_rgba(&out), &load_rgba(&sky_reference(shot.0)));
        if error > 1 {
            failures.push(format!(
                "{}: max channel error {error} (output: {})",
                shot.0,
                out.display()
            ));
        } else {
            let _ = std::fs::remove_file(out);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Regenerates the four time-of-day references from lavapipe renders.
/// Inspect the images before committing them.
#[test]
#[ignore = "regenerates tests/refs/sky_*.png"]
fn bless_sky_references() {
    for shot in SKY_SHOTS {
        render_sky_shot(shot, &sky_reference(shot.0));
    }
}

fn mean_rgb(img: &image::RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 3] {
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            for (s, c) in sum.iter_mut().zip(p) {
                *s += srgb_decode(c);
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

/// The sky differs between the four hours, is bright by day and dark at
/// night, and a mesh in front of it hides it.
#[test]
fn sky_changes_with_the_hour_and_meshes_occlude_it() {
    let (messages, _guard) = capture_messages();
    let top = |shot: SkyShot| {
        let out = out_path(&format!("{}-top.png", shot.0));
        render_sky_shot(shot, &out);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        // Upper part of the image: sky only.
        mean_rgb(&img, 0, 0, 256, 40)
    };
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let [dawn, noon, dusk, night] = SKY_SHOTS.map(top);
    assert!(luma(noon) > 3.0 * luma(night), "{noon:?} vs {night:?}");
    assert!(luma(noon) > 0.1 && luma(night) < 0.05);
    assert!(luma(dawn) < luma(noon) && luma(dusk) < luma(noon));

    // The cube at 9 m, 2.5 m to the left, hides the sky; above it is sky.
    let out = out_path("sky-occlusion.png");
    render_sky(12.0, 180.0, 20.0, 0.0, &out);
    let img = load_rgba(&out);
    let _ = std::fs::remove_file(out);
    let (camera, forward) = sky_camera(180.0, 20.0);
    let right = forward.cross(Vec3::Y);
    let cube = Vec3::new(0.0, 0.5, 0.0) + forward * 9.0 - right * 2.5;
    let (cx, cy) = pixel_of(&camera, cube, 256);
    let on_cube = img.get_pixel(cx, cy).0;
    let sky_above = img.get_pixel(cx, 10).0;
    assert_ne!(on_cube[..3], sky_above[..3]);
    assert!(sky_above[2] > sky_above[0], "blue sky");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Cloud cover 0 is a clear sky whatever the time; clouds move with the
/// wind (the animation time) and change the image.
#[test]
fn clouds_scroll_with_time_and_cover_zero_is_clear() {
    let (messages, _guard) = capture_messages();
    let shot = |cover: f32, time: f32| {
        let mut renderer = headless((128, 128));
        renderer.set_material_time(time);
        let (camera, _) = sky_camera(180.0, 40.0);
        let mut ambience = tod_ambience();
        ambience.sky.stars = 0.0;
        let world = RenderWorld {
            camera,
            ambience,
            sky: SkyParams {
                cloud_cover: cover,
                wind: Vec2::new(40.0, 10.0),
                ..SkyParams::default()
            },
            draw_sky: true,
            ..RenderWorld::default()
        };
        renderer.render_frame(&world).unwrap();
        let out = out_path(&format!("clouds-{cover}-{time}.png"));
        renderer.read_back_png(&out).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let clear_a = shot(0.0, 0.0);
    let clear_b = shot(0.0, 50.0);
    assert_eq!(
        max_channel_error(&clear_a, &clear_b),
        0,
        "clear sky is static"
    );
    let cloudy_a = shot(0.6, 0.0);
    let cloudy_b = shot(0.6, 50.0);
    assert!(
        max_channel_error(&cloudy_a, &clear_a) > 20,
        "clouds are visible"
    );
    assert!(max_channel_error(&cloudy_a, &cloudy_b) > 20, "clouds move");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// With the sun driven by the map time, the cascades follow it: the shadow
/// of a box lands on different ground pixels in the morning and afternoon.
#[test]
fn time_of_day_sun_moves_the_shadow() {
    let (messages, _guard) = capture_messages();
    let render = |time: f32| {
        let mut renderer = headless((256, 256));
        let instances = shadow_scene(&mut renderer);
        let mut world = shadow_world(instances);
        world.ambience.sun.source = SunSource::TimeOfDay;
        world.sky = SkyParams {
            time_h: time,
            latitude_deg: 40.0,
            ..SkyParams::default()
        };
        renderer.render_frame(&world).unwrap();
        let out = out_path(&format!("tod-shadow-{time}.png"));
        renderer.read_back_png(&out).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let morning = render(9.0);
    let afternoon = render(15.0);
    let differing = morning
        .pixels()
        .zip(afternoon.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(differing > 2000, "only {differing} pixels differ");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

// ---------------------------------------------------------------------------
// Weather (lot 2.7)
// ---------------------------------------------------------------------------

/// Reference name and weather of the three particle references.
fn weather_shots() -> [(&'static str, WeatherState); 3] {
    [
        (
            "weather_rain",
            WeatherState {
                rain: 1.0,
                humidity: 0.9,
                storm: 0.3,
                wind: Vec3::new(3.0, 0.0, 1.0),
                ..WeatherState::CLEAR
            },
        ),
        (
            "weather_snow",
            WeatherState {
                snow: 0.8,
                fog: 0.15,
                wind: Vec3::new(1.5, 0.0, 0.5),
                ..WeatherState::CLEAR
            },
        ),
        (
            "weather_sand",
            WeatherState {
                sand: 0.9,
                wind: Vec3::new(9.0, 0.0, 3.0),
                ..WeatherState::CLEAR
            },
        ),
    ]
}

/// Ground and three cubes below an afternoon sky, in `weather` at material
/// time `time`; `shelter` is loaded first when given.
fn render_weather(
    weather: WeatherState,
    shelter: Option<&ShelterMap>,
    time: f32,
    path: &Path,
) -> Renderer {
    let mut renderer = headless((256, 256));
    renderer.set_material_time(time);
    if let Some(map) = shelter {
        renderer.set_shelter_map(map).unwrap();
    }
    let mut mesh = |material: u8| {
        let (v, i) = cube_mesh(0.5, &[[230; 3]; 6], material);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    let (ground, cube) = (mesh(2), mesh(0));
    let (camera, forward) = sky_camera(180.0, 6.0);
    let right = forward.cross(Vec3::Y);
    let at = |ahead: f32, side: f32| Vec3::new(0.0, 0.5, 0.0) + forward * ahead + right * side;
    let mut instances = vec![Instance::new(
        ground,
        0,
        Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
            * Mat4::from_scale(Vec3::new(400.0, 1.0, 400.0)),
    )];
    for (ahead, side) in [(4.0, -1.5), (9.0, 1.2), (20.0, -4.0)] {
        instances.push(Instance::new(
            cube,
            0,
            Mat4::from_translation(at(ahead, side)),
        ));
    }
    let world = RenderWorld {
        camera,
        instances,
        ambience: tod_ambience(),
        sky: SkyParams {
            time_h: 14.0,
            latitude_deg: 45.0,
            ..SkyParams::default()
        },
        draw_sky: true,
        weather,
        ..RenderWorld::default()
    };
    // Twice, so both frame slots are used.
    for _ in 0..2 {
        renderer.render_frame(&world).unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    renderer
}

fn weather_reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

#[test]
fn rain_snow_and_sand_match_references() {
    let (messages, _guard) = capture_messages();
    let mut failures = Vec::new();
    for (name, weather) in weather_shots() {
        let out = out_path(&format!("{name}.png"));
        render_weather(weather, None, PROFILE_TIME, &out);
        let error = max_channel_error(&load_rgba(&out), &load_rgba(&weather_reference(name)));
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

/// Regenerates the three weather references from lavapipe renders. Inspect
/// the images before committing them.
#[test]
#[ignore = "regenerates tests/refs/weather_*.png"]
fn bless_weather_references() {
    for (name, weather) in weather_shots() {
        render_weather(weather, None, PROFILE_TIME, &weather_reference(name));
    }
}

/// The three kinds of weather look different from each other and from clear
/// weather, and the particles move with time.
#[test]
fn weather_kinds_differ_and_particles_move() {
    let (messages, _guard) = capture_messages();
    let render = |weather: WeatherState, time: f32, tag: &str| {
        let out = out_path(&format!("weather-{tag}.png"));
        render_weather(weather, None, time, &out);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let clear = render(WeatherState::CLEAR, 1.0, "clear");
    let [(_, rain), (_, snow), (_, sand)] = weather_shots();
    let images = [rain, snow, sand].map(|w| render(w, 1.0, "kind"));
    for (i, a) in images.iter().enumerate() {
        assert!(max_channel_error(a, &clear) > 30, "kind {i} is visible");
        for b in &images[i + 1..] {
            assert!(max_channel_error(a, b) > 30);
        }
    }
    let later = render(rain, 1.2, "later");
    let differing = images[0]
        .pixels()
        .zip(later.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(differing > 1000, "only {differing} pixels changed");
    // Rendering the same state twice gives the same image.
    assert_eq!(
        max_channel_error(&clear, &render(WeatherState::CLEAR, 1.0, "clear2")),
        0
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// A roof over one half of the world removes the particles there and only
/// there; a roof over everything removes them all.
#[test]
fn shelter_map_masks_particles_under_roofs() {
    let (messages, _guard) = capture_messages();
    let rain = WeatherState {
        rain: 1.0,
        ..WeatherState::CLEAR
    };
    let roof = |min_x: f32, max_x: f32| {
        ShelterMap::from_boxes(
            Vec2::new(-100.0, -100.0),
            200,
            200,
            1.0,
            &[(
                Vec3::new(min_x, -1.0, -100.0),
                Vec3::new(max_x, 50.0, 100.0),
            )],
        )
        .unwrap()
    };
    let render = |map: Option<&ShelterMap>, tag: &str| {
        let out = out_path(&format!("shelter-{tag}.png"));
        render_weather(rain, map, 1.0, &out);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let open = render(None, "open");
    let all = render(Some(&roof(-100.0, 100.0)), "all");
    // The camera looks along +Z, so +X is the left of the image.
    let left = render(Some(&roof(0.0, 100.0)), "left");
    let right = render(Some(&roof(-100.0, 0.0)), "right");
    // Compare columns away from the middle, where quads of either side spill over.
    let columns = |img: &image::RgbaImage, from: u32, to: u32| {
        let mut out = Vec::new();
        for y in 0..256 {
            for x in from..to {
                out.push(img.get_pixel(x, y).0);
            }
        }
        out
    };
    let (l0, l1, r0, r1) = (0, 100, 156, 256);
    // Under the roof: identical to a world without particles.
    assert_eq!(columns(&left, l0, l1), columns(&all, l0, l1));
    assert_eq!(columns(&right, r0, r1), columns(&all, r0, r1));
    // Outside the roof: particles are drawn.
    assert_ne!(columns(&left, r0, r1), columns(&all, r0, r1));
    assert_ne!(columns(&right, l0, l1), columns(&all, l0, l1));
    assert_eq!(columns(&left, r0, r1), columns(&open, r0, r1));
    assert_eq!(columns(&right, l0, l1), columns(&open, l0, l1));
    assert_ne!(columns(&open, l0, l1), columns(&all, l0, l1));
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Humidity darkens the ground and puddles reflect the sky.
#[test]
fn humidity_darkens_and_reflects() {
    let (messages, _guard) = capture_messages();
    let render = |humidity: f32| {
        let out = out_path("humidity.png");
        let weather = WeatherState {
            humidity,
            ..WeatherState::CLEAR
        };
        render_weather(weather, None, 1.0, &out);
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let dry = render(0.0);
    let damp = render(0.8);
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // Lower part of the image: ground only.
    let ground = |img: &image::RgbaImage| luma(mean_rgb(img, 0, 200, 256, 256));
    assert!(
        ground(&damp) < 0.85 * ground(&dry),
        "{} vs {}",
        ground(&damp),
        ground(&dry)
    );
    assert!(max_channel_error(&dry, &damp) > 20);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Storms flash and queue thunder with the right delay; calm weather does not.
#[test]
fn storm_flashes_and_queues_thunder() {
    use faset_render::{lightning_flash, strikes_between, thunder_delay, ThunderEvent};
    let (messages, _guard) = capture_messages();
    let strikes = strikes_between(0.0, 100.0, 1.0);
    assert!(strikes.len() > 5);
    let storm = WeatherState {
        storm: 1.0,
        ..WeatherState::CLEAR
    };
    let calm = WeatherState {
        storm: 0.2,
        ..WeatherState::CLEAR
    };
    let world = |weather: WeatherState| RenderWorld {
        weather,
        ambience: AmbienceProfile::builtin("night").unwrap(),
        draw_sky: true,
        ..RenderWorld::default()
    };
    let mut renderer = headless((64, 64));
    for (weather, expected) in [(storm, true), (calm, false)] {
        let world = world(weather);
        let mut events = Vec::new();
        // Time restarts: the jump back skips detection for one frame.
        renderer.set_material_time(0.0);
        renderer.render_frame(&world).unwrap();
        renderer.take_thunder_events();
        for k in 1..=200 {
            renderer.set_material_time(k as f32 * 0.5);
            renderer.render_frame(&world).unwrap();
            events.extend(renderer.take_thunder_events());
        }
        assert_eq!(!events.is_empty(), expected);
        if expected {
            let want: Vec<ThunderEvent> = strikes
                .iter()
                .map(|s| ThunderEvent {
                    delay_s: thunder_delay(s.distance_m),
                })
                .collect();
            assert_eq!(events, want);
            assert!(events.iter().all(|e| (0.8..=12.0).contains(&e.delay_s)));
        }
    }

    // The scene is brighter during a flash than a second later.
    let strike = strikes[0];
    let mut mean_at = |t: f32| {
        renderer.set_material_time(t);
        renderer.render_frame(&world(storm)).unwrap();
        let out = out_path("flash.png");
        renderer.read_back_png(&out).unwrap();
        let img = load_rgba(&out);
        let _ = std::fs::remove_file(out);
        mean_rgb(&img, 0, 0, 64, 64)
    };
    assert!(lightning_flash(strike.start_s, 1.0) > 0.99);
    let lit = mean_at(strike.start_s);
    let dark = mean_at(strike.start_s + 1.0);
    assert!(lit[2] > dark[2] + 0.1, "{lit:?} vs {dark:?}");
    assert_eq!(renderer.validation_message_count(), 0);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

// ---------------------------------------------------------------------------
// Water and underwater medium (lot 2.8)
// ---------------------------------------------------------------------------

fn look_at(eye: Vec3, target: Vec3) -> Camera {
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    Camera::new(view, perspective_reverse_z(60f32.to_radians(), 1.0, 0.1))
}

/// Renders a ground plane at y = 0 seen from `eye` with the given water.
fn render_water_scene(
    eye: Vec3,
    target: Vec3,
    water: Option<WaterParams>,
    caustics: f32,
    time: f32,
) -> image::RgbaImage {
    let mut renderer = headless((128, 128));
    renderer.set_material_time(time);
    let (v, i) = cube_mesh(0.5, &[[200; 3]; 6], 2);
    let ground = renderer.upload_mesh(&v, &i).unwrap();
    let mut ambience = AmbienceProfile::builtin("underwater").unwrap();
    ambience.water.caustics = caustics;
    let mut world = RenderWorld {
        camera: look_at(eye, target),
        instances: vec![Instance::new(
            ground,
            0,
            Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
                * Mat4::from_scale(Vec3::new(60.0, 1.0, 60.0)),
        )],
        ambience,
        water,
        ..RenderWorld::default()
    };
    world.update_underwater();
    renderer.render_frame(&world).unwrap();
    let path = out_path("water_scene.png");
    renderer.read_back_png(&path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    load_rgba(&path)
}

#[test]
fn underwater_flag_follows_the_camera_height() {
    let mut world = RenderWorld {
        water: Some(WaterParams {
            height: 2.0,
            waves: Vec::new(),
        }),
        ..RenderWorld::default()
    };
    for (y, expected) in [(1.9, true), (2.0, false), (6.0, false), (-3.0, true)] {
        world.camera = look_at(Vec3::new(0.0, y, 5.0), Vec3::new(0.0, y, 0.0));
        world.update_underwater();
        assert_eq!(world.underwater, expected, "camera y = {y}");
    }
    world.water = None;
    world.update_underwater();
    assert!(!world.underwater);
}

#[test]
fn water_surface_is_drawn_and_animates() {
    let (messages, _guard) = capture_messages();
    let eye = Vec3::new(0.0, 6.0, 10.0);
    let target = Vec3::new(0.0, 0.0, -10.0);
    let dry = render_water_scene(eye, target, None, 1.0, 1.0);
    let wet = render_water_scene(eye, target, Some(scene_water_at(0.5)), 1.0, 1.0);
    let later = render_water_scene(eye, target, Some(scene_water_at(0.5)), 1.0, 2.3);
    assert!(max_channel_error(&dry, &wet) > 20, "the surface is visible");
    let moved = wet
        .pixels()
        .zip(later.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 500, "only {moved} pixels changed with time");
    // Invalid water (fold-over) is ignored: the image is the dry one.
    let mut bad = scene_water_at(0.5);
    bad.waves[0].amplitude = 5.0;
    let ignored = render_water_scene(eye, target, Some(bad), 1.0, 1.0);
    assert_eq!(max_channel_error(&dry, &ignored), 0);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// The water surface writes motion vectors (camera motion), so temporal
/// anti-aliasing can follow it; a static camera gives none.
#[test]
fn water_surface_writes_motion_vectors() {
    let (messages, _guard) = capture_messages();
    let size = 64u32;
    let eye = Vec3::new(0.0, 3.0, 5.0);
    let target = Vec3::new(0.0, 0.0, -10.0);
    let moved = |dx: f32| {
        let mut renderer = headless((size, size));
        renderer.set_material_time(1.0);
        let mut camera = look_at(eye, target);
        // The camera moved `dx` metres right since the previous frame.
        let prev = look_at(
            eye - Vec3::new(dx, 0.0, 0.0),
            target - Vec3::new(dx, 0.0, 0.0),
        );
        camera.prev_view_proj = prev.view_proj();
        let world = RenderWorld {
            camera,
            water: Some(scene_water_at(0.5)),
            ..RenderWorld::default()
        };
        renderer.render_frame(&world).unwrap();
        let motion = renderer.read_back_motion().unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
        motion
    };
    // Rows below the horizon look at the water.
    let water_rows =
        |motion: &[[f32; 2]]| -> Vec<[f32; 2]> { motion[(size as usize * 48)..].to_vec() };
    let still = water_rows(&moved(0.0));
    assert!(still.iter().all(|m| *m == [0.0, 0.0]), "static camera");
    let moving = water_rows(&moved(0.5));
    assert!(
        moving.iter().all(|m| m[0] < -1e-4),
        "the water moves left on screen when the camera moves right"
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

fn scene_water_at(height: f32) -> WaterParams {
    WaterParams {
        height,
        ..scene_water()
    }
}

/// Below the surface the fog, caustics and particles change the image;
/// caustics only light what is under the water and move with time.
#[test]
fn underwater_medium_has_fog_and_animated_caustics() {
    let (messages, _guard) = capture_messages();
    let eye = Vec3::new(0.0, 3.0, 6.0);
    let target = Vec3::new(0.0, 0.0, -4.0);
    let water = Some(scene_water_at(8.0));
    let lit = render_water_scene(eye, target, water.clone(), 1.0, 1.0);
    let none = render_water_scene(eye, target, water.clone(), 0.0, 1.0);
    assert!(max_channel_error(&lit, &none) > 10, "caustics are visible");
    let later = render_water_scene(eye, target, water.clone(), 1.0, 2.0);
    assert!(max_channel_error(&lit, &later) > 10, "caustics animate");
    // The same scene above the surface (water below the ground) has none.
    let above = render_water_scene(eye, target, Some(scene_water_at(-2.0)), 1.0, 1.0);
    let above_none = render_water_scene(eye, target, Some(scene_water_at(-2.0)), 0.0, 1.0);
    assert_eq!(max_channel_error(&above, &above_none), 0);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

// ---------------------------------------------------------------------------
// Post-processing chain (lot 2.9)
// ---------------------------------------------------------------------------

/// Parameters of the reference images: every effect visibly on.
fn post_reference_params() -> PostParams {
    PostParams {
        exposure: 0.9,
        bloom_threshold: 1.0,
        bloom_knee: 0.3,
        bloom_intensity: 0.25,
        saturation: 1.15,
        hue_shift: 0.1,
        gamma: [1.1; 3],
        contour_strength: 0.6,
        vignette: 0.3,
        chromatic_aberration: 0.003,
        grain: 0.03,
        ripple: 0.004,
        damage_flash: 0.08,
        speed_lines: 0.12,
        ..PostParams::default()
    }
}

/// Ground, three cubes and a day sky; `post` replaces the derived parameters.
fn render_post(
    settings: Option<PostSettings>,
    post: Option<PostParams>,
    path: &Path,
) -> image::RgbaImage {
    let mut renderer = headless((128, 128));
    renderer.set_material_time(PROFILE_TIME);
    renderer.set_post_settings(settings).unwrap();
    let mut mesh = |material: u8| {
        let (v, i) = cube_mesh(0.5, &[[230; 3]; 6], material);
        renderer.upload_mesh(&v, &i).unwrap()
    };
    let (ground, cube) = (mesh(2), mesh(0));
    let (camera, forward) = sky_camera(180.0, 6.0);
    let right = forward.cross(Vec3::Y);
    let at = |ahead: f32, side: f32| Vec3::new(0.0, 0.5, 0.0) + forward * ahead + right * side;
    let mut instances = vec![Instance::new(
        ground,
        0,
        Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
            * Mat4::from_scale(Vec3::new(400.0, 1.0, 400.0)),
    )];
    for (ahead, side) in [(3.0, -1.0), (6.0, 1.0), (12.0, -3.0)] {
        instances.push(Instance::new(
            cube,
            0,
            Mat4::from_translation(at(ahead, side)),
        ));
    }
    let world = RenderWorld {
        camera,
        instances,
        ambience: tod_ambience(),
        sky: SkyParams {
            time_h: 14.0,
            latitude_deg: 45.0,
            ..SkyParams::default()
        },
        draw_sky: true,
        post,
        ..RenderWorld::default()
    };
    // Twice, so both frame slots are used.
    for _ in 0..2 {
        renderer.render_frame(&world).unwrap();
    }
    renderer.read_back_png(path).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    load_rgba(path)
}

/// (name, settings) of the full chain and of one image per toggle.
fn post_shots() -> Vec<(&'static str, PostSettings)> {
    let all = PostSettings::default();
    vec![
        ("post_full", all),
        (
            "post_contours_off",
            PostSettings {
                contours: false,
                ..all
            },
        ),
        (
            "post_bloom_off",
            PostSettings {
                bloom: false,
                ..all
            },
        ),
        (
            "post_tonemap_off",
            PostSettings {
                tonemap: false,
                ..all
            },
        ),
        (
            "post_grading_off",
            PostSettings {
                grading: false,
                ..all
            },
        ),
        (
            "post_screen_effects_off",
            PostSettings {
                screen_effects: false,
                ..all
            },
        ),
        ("post_fxaa_off", PostSettings { fxaa: false, ..all }),
    ]
}

fn post_reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

#[test]
fn post_chain_matches_references() {
    let (messages, _guard) = capture_messages();
    let mut failures = Vec::new();
    for (name, settings) in post_shots() {
        let out = out_path(&format!("{name}.png"));
        let got = render_post(Some(settings), Some(post_reference_params()), &out);
        let error = max_channel_error(&got, &load_rgba(&post_reference(name)));
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

/// Regenerates the post references from lavapipe renders. Inspect the images
/// before committing them.
#[test]
#[ignore = "regenerates tests/refs/post_*.png"]
fn bless_post_references() {
    for (name, settings) in post_shots() {
        render_post(
            Some(settings),
            Some(post_reference_params()),
            &post_reference(name),
        );
    }
}

#[test]
fn each_toggle_changes_the_image() {
    let (messages, _guard) = capture_messages();
    let out = out_path("post_toggle.png");
    let params = Some(post_reference_params());
    let full = render_post(Some(PostSettings::default()), params, &out);
    for (name, settings) in post_shots().into_iter().skip(1) {
        let img = render_post(Some(settings), params, &out);
        assert!(
            max_channel_error(&full, &img) > 1,
            "turning off {name} changes nothing"
        );
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn all_effects_off_copies_the_hdr_buffer() {
    let (messages, _guard) = capture_messages();
    let out = out_path("post_none.png");
    let plain = render_post(None, None, &out);
    let none = render_post(
        Some(PostSettings::NONE),
        Some(post_reference_params()),
        &out,
    );
    assert_eq!(max_channel_error(&plain, &none), 0);
    // Default parameters and no tonemap or FXAA: the grading pass alone is
    // the identity.
    let neutral = PostSettings {
        tonemap: false,
        fxaa: false,
        ..PostSettings::default()
    };
    let identity = render_post(Some(neutral), Some(PostParams::default()), &out);
    assert!(max_channel_error(&plain, &identity) <= 1);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn zero_intensity_and_false_flags_pass_through() {
    let (messages, _guard) = capture_messages();
    let out = out_path("post_zero.png");
    let base = PostSettings {
        tonemap: false,
        fxaa: false,
        ..PostSettings::default()
    };
    let neutral = PostParams::default();
    let baseline = render_post(Some(base), Some(neutral), &out);
    let active: [(&str, PostParams); 7] = [
        (
            "contours",
            PostParams {
                contour_strength: 0.8,
                ..neutral
            },
        ),
        (
            "bloom",
            PostParams {
                bloom_intensity: 1.0,
                bloom_threshold: 0.2,
                ..neutral
            },
        ),
        (
            "vignette",
            PostParams {
                vignette: 0.5,
                ..neutral
            },
        ),
        (
            "chromatic aberration",
            PostParams {
                chromatic_aberration: 0.01,
                ..neutral
            },
        ),
        (
            "grain",
            PostParams {
                grain: 0.1,
                ..neutral
            },
        ),
        (
            "ripple",
            PostParams {
                ripple: 0.01,
                ..neutral
            },
        ),
        (
            "speed lines",
            PostParams {
                speed_lines: 1.0,
                damage_flash: 0.4,
                ..neutral
            },
        ),
    ];
    for (name, params) in active {
        let on = render_post(Some(base), Some(params), &out);
        assert!(
            max_channel_error(&baseline, &on) > 1,
            "{name} has no effect at a positive intensity"
        );
        // The same intensity with its flag off is the baseline again.
        let flag_off = match name {
            "contours" => PostSettings {
                contours: false,
                ..base
            },
            "bloom" => PostSettings {
                bloom: false,
                ..base
            },
            _ => PostSettings {
                screen_effects: false,
                ..base
            },
        };
        let off = render_post(Some(flag_off), Some(params), &out);
        assert!(
            max_channel_error(&baseline, &off) <= 1,
            "{name} still shows with its flag off"
        );
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Render of a flat colour: no mesh, no sky.
fn render_flat(settings: Option<PostSettings>, color: [f32; 4], exposure: f32) -> image::RgbaImage {
    let mut renderer = headless((64, 64));
    renderer.set_post_settings(settings).unwrap();
    let world = RenderWorld {
        clear_color: color,
        post: Some(PostParams {
            exposure,
            ..PostParams::default()
        }),
        ..RenderWorld::default()
    };
    renderer.render_frame(&world).unwrap();
    let out = out_path("post_flat.png");
    renderer.read_back_png(&out).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    load_rgba(&out)
}

#[test]
fn fxaa_leaves_a_flat_image_unchanged() {
    let (messages, _guard) = capture_messages();
    let color = [0.2, 0.45, 0.8, 1.0];
    let plain = render_flat(None, color, 1.0);
    let fxaa = PostSettings {
        fxaa: true,
        ..PostSettings::NONE
    };
    let smoothed = render_flat(Some(fxaa), color, 1.0);
    assert_eq!(max_channel_error(&plain, &smoothed), 0);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn tonemap_matches_the_cpu_curve() {
    let (messages, _guard) = capture_messages();
    let tonemap = PostSettings {
        tonemap: true,
        ..PostSettings::NONE
    };
    for (color, exposure) in [
        ([0.2, 0.45, 0.8, 1.0], 1.0),
        ([0.05, 0.5, 2.0, 1.0], 1.5),
        // Above x ~ 7.25 the plan's curve exceeds 1: the output is clamped.
        ([12.0, 0.0, 0.3, 1.0], 1.0),
    ] {
        let img = render_flat(Some(tonemap), color, exposure);
        let expected = faset_render::tonemap_pixel([color[0], color[1], color[2]], exposure);
        let got = img.get_pixel(32, 32).0;
        for c in 0..3 {
            let e = srgb_encode(expected[c]);
            assert!(
                got[c].abs_diff(e) <= 1,
                "channel {c} of {color:?}: {} vs {e}",
                got[c]
            );
        }
    }
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn screen_effects_follow_the_world_and_survive_a_resize() {
    let (messages, _guard) = capture_messages();
    let mut renderer = headless((96, 64));
    renderer
        .set_post_settings(Some(PostSettings::default()))
        .unwrap();
    let mut world = RenderWorld {
        clear_color: [0.3, 0.3, 0.3, 1.0],
        ..RenderWorld::default()
    };
    renderer.render_frame(&world).unwrap();
    world.screen = ScreenEffects {
        damage_flash: 0.8,
        speed_lines: 0.0,
    };
    renderer.render_frame(&world).unwrap();
    // A resize recreates the targets.
    renderer.resize(160, 96);
    renderer.render_frame(&world).unwrap();
    renderer.set_post_settings(None).unwrap();
    renderer.render_frame(&world).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}
