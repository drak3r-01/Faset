//! The loadout screen's 3D character preview on Mesa lavapipe (run by
//! `cargo xtask render-test`): every class renders, the character is on
//! screen and the validation layers stay silent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_client::{CharacterPreview, Profile};
use faset_mode_fps::class::Catalog;
use faset_mode_fps::Loadout;
use faset_render::{RenderConfig, Renderer};
use faset_ui::{LoadoutSelection, PreviewRequest};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

const SIZE: (u32, u32) = (640, 360);

struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn selection(loadout: &Loadout, body: u8, accent: u8) -> LoadoutSelection {
    LoadoutSelection {
        class: loadout.class.clone(),
        primary: loadout.primary.clone(),
        secondary: loadout.secondary.clone(),
        melee: loadout.melee.clone(),
        gadget: loadout.gadget.clone(),
        body_colour: body,
        accent_colour: accent,
    }
}

#[test]
fn every_class_preview_renders_without_validation_messages() {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(VulkanMessages(count.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut renderer = Renderer::new_headless(&RenderConfig {
        size: SIZE,
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    let preview = CharacterPreview::upload(&mut renderer).expect("preview meshes upload");
    let out = std::env::temp_dir().join(format!("{}-loadout_preview.png", std::process::id()));
    let aspect = SIZE.0 as f32 / SIZE.1 as f32;
    for (i, class) in Catalog::builtin().classes().iter().enumerate() {
        let mut loadout = Loadout::for_class(class);
        if class.id == "prankster" {
            loadout.melee = Some("baguette".into());
            loadout.gadget = Some("bubble_gun".into());
        }
        let request = PreviewRequest {
            selection: selection(&loadout, (i % 8) as u8, ((i + 3) % 8) as u8),
        };
        let world = preview.render_world(&request, i as f32 * 0.7, aspect);
        assert!(world.instances.len() >= 9);
        for _ in 0..3 {
            renderer.render_frame(&world).unwrap();
        }
    }
    renderer.read_back_png(&out).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    let got = image::open(&out).unwrap().to_rgba8();
    let clear = got.get_pixel(0, 0).0;
    let drawn = got.pixels().filter(|p| p.0 != clear).count();
    assert!(
        drawn > got.pixels().len() / 20,
        "the character is on screen"
    );
    let _ = std::fs::remove_file(out);
    assert_eq!(count.load(Ordering::SeqCst), 0, "validation messages");
    // The default profile's class is previewable too.
    let default = Profile::default();
    assert_eq!(default.current().loadout, Loadout::trooper());
}
