//! The example map, built by the client and rendered headless on Mesa
//! lavapipe (run by `cargo xtask render-test`), against a reference image.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_client::load_map;
use faset_render::glam::Vec3;
use faset_render::{jitter, perspective_reverse_z, Camera, RenderConfig, RenderWorld, Renderer};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

#[path = "lavapipe/hud_captures.rs"]
mod hud_captures;
#[path = "lavapipe/loadout_preview.rs"]
mod loadout_preview;
#[path = "lavapipe/map_looks.rs"]
mod map_looks;
#[path = "lavapipe/match_scene.rs"]
mod match_scene;

const REFERENCE: &str = "tests/refs/example_map.png";
const EXAMPLE: &str = include_str!("../../../assets/testdata/example.ron");
const SIZE: (u32, u32) = (640, 360);

/// Counts events logged by the renderer's Vulkan debug callback.
struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn render_example(path: &Path) -> usize {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(VulkanMessages(count.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    {
        let mut renderer = Renderer::new_headless(&RenderConfig {
            size: SIZE,
            ..RenderConfig::default()
        })
        .expect("headless renderer on lavapipe");
        let map = load_map(EXAMPLE.as_bytes()).expect("example map loads");
        let instances = map.upload(&mut renderer).expect("meshes upload");
        assert!(!instances.is_empty());
        let view = faset_render::glam::camera::rh::view::look_at_mat4(
            Vec3::new(-8.0, 38.0, 46.0),
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::Y,
        );
        let proj = perspective_reverse_z(55f32.to_radians(), SIZE.0 as f32 / SIZE.1 as f32, 0.1);
        for frame in 0..4u32 {
            let mut camera = Camera::new(view, proj);
            camera.jitter_px = jitter(frame);
            let world = RenderWorld {
                clear_color: [0.25, 0.35, 0.5, 1.0],
                camera,
                instances: instances.clone(),
                ..RenderWorld::default()
            };
            renderer.render_frame(&world).unwrap();
        }
        renderer.read_back_png(path).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    count.load(Ordering::SeqCst)
}

fn out_path() -> PathBuf {
    let dir = std::env::temp_dir().join("faset-client-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-example_map.png", std::process::id()))
}

fn reference_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(REFERENCE)
}

#[test]
fn example_map_matches_reference() {
    let out = out_path();
    let messages = render_example(&out);
    assert_eq!(messages, 0, "validation messages");
    let got = image::open(&out).unwrap().to_rgba8();
    let reference = image::open(reference_path())
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
        error <= 1,
        "max channel error {error} > 1 (output: {})",
        out.display()
    );
    // The map is not a blank image.
    let clear = got.get_pixel(0, 0).0;
    let drawn = got.pixels().filter(|p| p.0 != clear).count();
    assert!(drawn > got.pixels().len() / 4);
    let _ = std::fs::remove_file(out);
}

/// Regenerates the reference from a lavapipe render. Inspect before committing.
#[test]
#[ignore = "regenerates tests/refs/example_map.png"]
fn bless_example_map_reference() {
    render_example(&reference_path());
}
