//! A live match drawn on Mesa lavapipe (run by `cargo xtask render-test`):
//! the map, the other players, the weapon in hand and the HUD, from a real
//! integrated server with bots, and the validation layers stay silent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use faset_client::{
    App, AppState, FpsHud, GameInput, HudOptions, LocalFrame, MapCatalogue, MatchScene, SceneFrame,
};
use faset_core::bindings::Bindings;
use faset_core::settings::{ControlSettings, WeaponPosition};
use faset_minigame_api::Registry;
use faset_mode_fps::register_modes;
use faset_render::glam::Mat4;
use faset_render::{
    perspective_reverse_z, AmbienceProfile, Camera, RenderConfig, RenderWorld, Renderer,
};
use faset_ui::{FrameInput, PlayRequest, Theme, Ui, UiCommand, UiConfig};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

const SIZE: (u32, u32) = (800, 450);

struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn a_live_match_renders_with_players_weapon_and_hud() {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(VulkanMessages(count.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut registry = Registry::new();
    register_modes(&mut registry).unwrap();
    let dir = std::env::temp_dir().join(format!("faset-match-scene-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(registry, MapCatalogue::scan(&dir));
    let (bindings, controls) = (Bindings::default(), ControlSettings::default());
    let mut game = GameInput::new();
    let mut last = Instant::now();
    let mut frame = |app: &mut App| {
        std::thread::sleep(Duration::from_millis(8));
        let dt = last.elapsed();
        last = Instant::now();
        app.update(dt, &mut |t| game.command(t, &bindings, &controls));
    };
    frame(&mut app);
    app.handle(vec![UiCommand::StartMatch(PlayRequest {
        mode: "fps.ffa".to_owned(),
        bots: 3,
        max_players: 8,
        ..PlayRequest::default()
    })]);
    let start = Instant::now();
    while app.stack().top() != AppState::InGame
        || app.session().is_none_or(|s| s.entity_count() < 4)
    {
        assert!(start.elapsed() < Duration::from_secs(30), "match not ready");
        frame(&mut app);
    }

    let mut renderer = Renderer::new_headless(&RenderConfig {
        size: SIZE,
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    let mut ui =
        Ui::new(&mut renderer, UiConfig::default(), Theme::default()).expect("UI on lavapipe");
    let map = app.take_map().expect("the map of the match");
    let map_instances = map.upload(&mut renderer).expect("the map uploads");
    let mut scene = MatchScene::upload(&mut renderer).expect("the scene uploads");

    // Let the bots spread out, then draw from the player's eye.
    for _ in 0..120 {
        frame(&mut app);
    }
    let session = app.session_mut().expect("a session");
    let state = session
        .prediction()
        .and_then(|p| p.controller_state())
        .expect("a predicted player");
    let eye = state.pos + faset_render::glam::Vec3::Y * 0.7;
    let proj = perspective_reverse_z(80f32.to_radians(), SIZE.0 as f32 / SIZE.1 as f32, 0.1);
    let view = Mat4::from_translation(eye).inverse();
    let camera = Camera::new(view, proj);
    let (_, orientation, eye) = view.inverse().to_scale_rotation_translation();
    let scene_frame = SceneFrame {
        eye,
        orientation,
        dt: 0.016,
        speed: 0.0,
        grounded: true,
        weapon_position: WeaponPosition::Right,
        look: game.look(),
    };
    let extra = scene.instances(session, &scene_frame);
    assert!(
        extra.world.len() + extra.viewmodel.len() >= 3,
        "the weapon in hand and the other players are drawn: {} + {} instances",
        extra.world.len(),
        extra.viewmodel.len()
    );
    assert!(!extra.viewmodel.is_empty(), "the weapon is a view model");
    let mut instances = map_instances;
    instances.extend(extra.world);
    let viewmodel = extra.viewmodel;
    let hud_frame = LocalFrame::default();
    let model = session
        .view_mut()
        .hud_model(&hud_frame, 0.016)
        .expect("a HUD model");
    let mut hud = FpsHud::new(model, 80f32.to_radians()).with_options(HudOptions::default());

    let world = RenderWorld {
        camera,
        instances,
        viewmodel,
        ambience: AmbienceProfile::builtin("day").unwrap(),
        draw_sky: true,
        ..RenderWorld::default()
    };
    let out = std::env::temp_dir().join(format!("{}-match_scene.png", std::process::id()));
    for second in 20..23 {
        ui.begin_frame(FrameInput {
            size_px: SIZE,
            time: f64::from(second),
            ..FrameInput::default()
        });
        ui.draw(&mut hud);
        renderer.render_frame(&world).unwrap();
    }
    renderer.read_back_png(&out).unwrap();
    assert_eq!(renderer.validation_message_count(), 0);
    let got = image::open(&out).unwrap().to_rgba8();
    let first = got.get_pixel(0, 0).0;
    assert!(got.pixels().any(|p| p.0 != first), "something is drawn");
    if let Ok(keep) = std::env::var("FASET_MATCH_SCENE_PNG") {
        std::fs::copy(&out, &keep).unwrap_or_else(|e| panic!("{e}: {out:?} to {keep}"));
    }
    let _ = std::fs::remove_file(out);
    assert_eq!(count.load(Ordering::SeqCst), 0, "validation messages");
}
