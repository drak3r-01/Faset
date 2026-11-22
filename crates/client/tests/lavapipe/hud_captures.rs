//! FPS HUD captures on Mesa lavapipe (run by `cargo xtask render-test`), at
//! 16:9 and 16:10, against reference images in `tests/refs/hud_*.png`.
//!
//! Every capture asserts zero validation messages. Regenerate the references
//! with `bless_hud_references` and inspect them before committing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use faset_client::FpsHud;
use faset_core::NetId;
use faset_mode_fps::{
    ChoiceMoment, ClassChoice, ClassOption, HitZone, HudInput, HudModel, HudState, PickupKind,
    ScoreRow, ServerMsg,
};
use faset_render::{RenderConfig, RenderWorld, Renderer};
use faset_ui::{FrameInput, Theme, Ui, UiConfig};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// 16:9 and 16:10 display sizes.
const SIZES: [(&str, (u32, u32)); 2] = [("16x9", (800, 450)), ("16x10", (800, 500))];
const FOV_Y_DEG: f32 = 75.0;

struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn reference(name: &str, size: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/hud_{name}_{size}.png"))
}

fn out_path(name: &str, size: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("faset-client-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-hud_{name}_{size}.png", std::process::id()))
}

fn rgba(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()))
        .to_rgba8()
}

fn max_error(a: &image::RgbaImage, b: &image::RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions(), "image sizes differ");
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// Renders `hud` over a flat sky-coloured scene and writes the PNG.
fn capture(hud: &mut FpsHud, size: (u32, u32), path: &Path) -> usize {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(VulkanMessages(count.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    {
        let mut renderer = Renderer::new_headless(&RenderConfig {
            size,
            ..RenderConfig::default()
        })
        .expect("headless renderer on lavapipe");
        let mut ui =
            Ui::new(&mut renderer, UiConfig::default(), Theme::default()).expect("UI on lavapipe");
        let world = RenderWorld {
            clear_color: [0.32, 0.42, 0.52, 1.0],
            ..RenderWorld::default()
        };
        for second in 20..23 {
            ui.begin_frame(FrameInput {
                size_px: size,
                time: f64::from(second),
                ..FrameInput::default()
            });
            ui.draw(hud);
            renderer.render_frame(&world).unwrap();
        }
        renderer.read_back_png(path).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    count.load(Ordering::SeqCst)
}

const LOCAL: NetId = NetId(1);

fn names() -> std::collections::BTreeMap<NetId, String> {
    [(1, "You"), (2, "Rook"), (3, "Vex"), (4, "Mako")]
        .into_iter()
        .map(|(id, n)| (NetId(id), n.to_owned()))
        .collect()
}

fn rows() -> Vec<ScoreRow> {
    let row = |name: &str, score, kills, deaths, local| ScoreRow {
        name: name.to_owned(),
        score,
        kills,
        deaths,
        local,
    };
    vec![
        row("Vex", 14, 14, 6, false),
        row("You", 11, 11, 7, true),
        row("Rook", 9, 9, 9, false),
        row("Mako", 4, 4, 12, false),
    ]
}

fn base_input() -> HudInput {
    HudInput {
        player: Some(LOCAL),
        names: names(),
        weapon: "Pulse rifle".to_owned(),
        ammo: Some((24, 90)),
        speed: 7.0,
        sigma: 0.012,
        time_left: Some(277.0),
        scores: rows(),
        ..HudInput::default()
    }
}

fn fov() -> f32 {
    FOV_Y_DEG.to_radians()
}

fn standing() -> HudModel {
    let mut s = HudState::default();
    s.set_input(base_input());
    s.model(
        Some((100, 35, true)),
        vec![(PickupKind::Speed, 6.4)],
        vec![],
    )
}

fn sliding() -> HudModel {
    let mut s = HudState::default();
    s.set_input(HudInput {
        speed: 11.5,
        sigma: 0.035,
        ammo: Some((11, 90)),
        ..base_input()
    });
    s.model(
        Some((72, 0, true)),
        vec![(PickupKind::Damage, 12.0), (PickupKind::Speed, 2.1)],
        vec![],
    )
}

fn low_health() -> HudModel {
    let mut s = HudState::default();
    s.set_input(HudInput {
        ammo: Some((2, 14)),
        ..base_input()
    });
    s.model(Some((18, 0, true)), vec![], vec![])
}

fn kill_feed() -> HudModel {
    let mut s = HudState::default();
    s.set_input(base_input());
    for (a, v) in [(2, 3), (1, 4), (3, 1)] {
        s.on_msg(&ServerMsg::Kill {
            attacker: NetId(a),
            victim: NetId(v),
        });
    }
    s.on_msg(&ServerMsg::Hit {
        attacker: LOCAL,
        victim: NetId(4),
        amount: 62.0,
        zone: HitZone::Head,
    });
    s.on_msg(&ServerMsg::Hit {
        attacker: LOCAL,
        victim: NetId(4),
        amount: 28.0,
        zone: HitZone::Body,
    });
    s.advance(0.1);
    let mut model = s.model(Some((64, 10, true)), vec![], vec![]);
    model.kill_marker = 0.7;
    model
}

fn scoreboard() -> HudModel {
    let mut s = HudState::default();
    s.set_input(HudInput {
        show_scoreboard: true,
        ..base_input()
    });
    s.model(Some((100, 0, true)), vec![], vec![])
}

fn class_choice() -> HudModel {
    let option = |name: &str, summary: &str| ClassOption {
        name: name.to_owned(),
        summary: summary.to_owned(),
    };
    let mut s = HudState::default();
    s.set_input(HudInput {
        class_choice: Some(ClassChoice {
            moment: ChoiceMoment::Death,
            options: vec![
                option("Assault", "100 hp, speed x1.0, pulse rifle"),
                option("Sprinter", "80 hp, speed x1.15, dart pistol"),
                option("Marksman", "90 hp, speed x0.95, long rifle"),
                option("Bruiser", "130 hp, speed x0.9, scatter gun"),
            ],
            selected: 1,
        }),
        ..base_input()
    });
    s.model(Some((0, 0, false)), vec![], vec![])
}

fn cases() -> Vec<(&'static str, HudModel)> {
    vec![
        ("standing", standing()),
        ("sliding", sliding()),
        ("low_health", low_health()),
        ("kill_feed", kill_feed()),
        ("scoreboard", scoreboard()),
        ("class_choice", class_choice()),
    ]
}

#[test]
fn hud_captures_match_their_references() {
    for (name, model) in cases() {
        for (label, size) in SIZES {
            let out = out_path(name, label);
            let mut hud = FpsHud::new(model.clone(), fov());
            assert_eq!(
                capture(&mut hud, size, &out),
                0,
                "validation: {name} {label}"
            );
            let got = rgba(&out);
            let error = max_error(&got, &rgba(&reference(name, label)));
            assert!(
                error <= 2,
                "hud_{name}_{label}: max channel error {error} (output: {})",
                out.display()
            );
            // The HUD drew something over the flat background.
            let clear = got.get_pixel(0, got.height() / 2).0;
            let drawn = got.pixels().filter(|p| p.0 != clear).count();
            assert!(drawn > 200, "hud_{name}_{label} is blank");
            let _ = std::fs::remove_file(out);
        }
    }
}

#[test]
fn a_wider_dispersion_cone_widens_the_reticle_in_the_capture() {
    let size = SIZES[0].1;
    let render = |sigma: f32| {
        let mut s = HudState::default();
        s.set_input(HudInput {
            sigma,
            ..base_input()
        });
        let model = s.model(Some((100, 0, true)), vec![], vec![]);
        let out = out_path(&format!("sigma{}", (sigma * 1000.0) as u32), "t");
        capture(&mut FpsHud::new(model, fov()), size, &out);
        let img = rgba(&out);
        let _ = std::fs::remove_file(out);
        img
    };
    let (w, h) = (size.0, size.1);
    // Distance of the first non-background pixel right of the centre.
    let first_tick = |img: &image::RgbaImage| {
        let clear = img.get_pixel(w / 2 + 60, h / 2 - 1).0;
        (3..80)
            .find(|dx| img.get_pixel(w / 2 + dx, h / 2).0 != clear)
            .expect("a reticle tick")
    };
    let narrow = first_tick(&render(0.004));
    let wide = first_tick(&render(0.04));
    assert!(wide > narrow + 5, "narrow {narrow}, wide {wide}");
}

/// Regenerates the references. Inspect the images before committing them.
#[test]
#[ignore = "regenerates tests/refs/hud_*.png"]
fn bless_hud_references() {
    for (name, model) in cases() {
        for (label, size) in SIZES {
            let mut hud = FpsHud::new(model.clone(), fov());
            capture(&mut hud, size, &reference(name, label));
        }
    }
}
