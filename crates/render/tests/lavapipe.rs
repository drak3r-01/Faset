//! Headless renderer tests on Mesa lavapipe (run by `cargo xtask render-test`).
//!
//! Every test asserts zero validation messages, counted twice: by the
//! renderer's debug messenger, and by a tracing layer that also sees messages
//! emitted while the device and instance are destroyed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_render::{RenderConfig, RenderError, RenderWorld, Renderer};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// Linear clear colour of the reference image.
const CLEAR: [f32; 4] = [0.25, 0.5, 0.75, 1.0];
const REFERENCE: &str = "tests/refs/clear_256.png";

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

#[test]
fn clear_256_matches_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("clear_256.png");
    {
        let mut renderer = headless((256, 256));
        let world = RenderWorld { clear_color: CLEAR };
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
        let world = RenderWorld { clear_color: CLEAR };
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
