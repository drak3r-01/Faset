//! Manual check of the upscalers (the J2 scene): a low-poly scene with sky,
//! optional rain and an orbiting camera, shown through the chosen upscaler.
//!
//! `window [--upscaler native|taa|fsr1|fsr3|dlss] [--quality QUALITY]
//!         [--no-vsync] [--seconds N] [--weather]`
//!
//! QUALITY is `quality`, `balanced`, `performance`, `ultra`, `native` or
//! `free:S` (a render scale S in [0.5, 1], Native and FSR 1 only). Defaults:
//! FSR 1 at `balanced`. N = 0 runs until the window is closed. `fsr3` needs
//! `amd_fidelityfx_vk.dll` next to the executable (Windows x86_64); `dlss`
//! needs a build with the `dlss` feature, `nvngx_dlss.dll` next to the
//! executable (the build copies it there) and an RTX GPU. Without them the
//! example falls back to FSR 1 and logs why.

use std::time::{Duration, Instant};

use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    cube_mesh, perspective_reverse_z, AmbienceProfile, Camera, Instance, MeshId, PostSettings,
    RenderConfig, RenderWorld, Renderer, SkyParams, WeatherState, CUBE_FACE_TINTS,
};
use faset_upscale::{Quality, Registry, UpscalerKind};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    cfg: RenderConfig,
    kind: UpscalerKind,
    quality: Quality,
    rain: bool,
    limit: Option<Duration>,
    start: Instant,
    last_log: Instant,
    meshes: Option<(MeshId, MeshId)>,
    prev_view_proj: Option<Mat4>,
    // Field order: the renderer is dropped before the window.
    renderer: Option<Renderer>,
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Faset upscaler check")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.cfg.size.0,
                self.cfg.size.1,
            ));
        let window = event_loop.create_window(attrs).expect("create window");
        match Renderer::new_windowed(&window, &self.cfg) {
            Ok(mut r) => {
                let registry = Registry::builtin();
                let gpu = r.gpu_handles();
                tracing::info!(modes = ?registry.available(&gpu), "available upscalers");
                match registry.create_pass(self.kind, self.quality, &gpu) {
                    Ok((kind, pass)) => {
                        tracing::info!(upscaler = kind.name(), quality = ?self.quality, "upscaler on");
                        if let Err(e) = r.set_upscaler(Some(pass)) {
                            tracing::error!("cannot install the upscaler: {e}");
                        }
                    }
                    Err(e) => tracing::error!("no upscaler: {e}"),
                }
                if let Err(e) = r.set_post_settings(Some(PostSettings::default())) {
                    tracing::error!("post chain unavailable: {e}");
                }
                let mut mesh = |tints: &[[u8; 3]; 6], material: u8| {
                    let (v, i) = cube_mesh(0.5, tints, material);
                    r.upload_mesh(&v, &i).ok()
                };
                self.meshes = mesh(&[[200; 3]; 6], 2).zip(mesh(&CUBE_FACE_TINTS, 0));
                self.renderer = Some(r);
            }
            Err(e) => {
                tracing::error!("renderer creation failed: {e}");
                event_loop.exit();
            }
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl App {
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(renderer), Some((ground, cube))) = (&mut self.renderer, self.meshes) else {
            return;
        };
        let t = self.start.elapsed().as_secs_f32();
        let (w, h) = renderer.size();
        let eye = Vec3::new(10.0 * (0.15 * t).cos(), 3.5, 10.0 * (0.15 * t).sin());
        let view = faset_render::glam::camera::rh::view::look_at_mat4(
            eye,
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::Y,
        );
        let proj =
            perspective_reverse_z(60f32.to_radians(), w.max(1) as f32 / h.max(1) as f32, 0.1);
        let mut camera = Camera::new(view, proj);
        if let Some(prev) = self.prev_view_proj {
            camera.prev_view_proj = prev;
        }
        camera.jitter_px = renderer.upscaler_jitter(renderer.frames_rendered() as u32);
        self.prev_view_proj = Some(camera.view_proj());
        let mut instances = vec![Instance::new(
            ground,
            0,
            Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0))
                * Mat4::from_scale(Vec3::new(200.0, 1.0, 200.0)),
        )];
        for k in 0..60 {
            let (x, z) = ((k % 10) as f32 * 2.2 - 10.0, (k / 10) as f32 * 2.2 - 6.0);
            let s = 0.6 + 0.12 * (k % 5) as f32;
            instances.push(Instance::new(
                cube,
                0,
                Mat4::from_translation(Vec3::new(x, 0.5 * s - 0.5 + 0.5, z))
                    * Mat4::from_rotation_y(0.4 * k as f32)
                    * Mat4::from_scale(Vec3::splat(s)),
            ));
        }
        renderer.set_material_time(t);
        let world = RenderWorld {
            camera,
            instances,
            ambience: AmbienceProfile::builtin("day").expect("built-in profile"),
            sky: SkyParams {
                time_h: 15.0,
                latitude_deg: 45.0,
                ..SkyParams::default()
            },
            draw_sky: true,
            weather: if self.rain {
                WeatherState {
                    rain: 1.0,
                    ..WeatherState::CLEAR
                }
            } else {
                WeatherState::CLEAR
            },
            ..RenderWorld::default()
        };
        if let Err(e) = renderer.render_frame(&world) {
            tracing::error!("frame failed: {e}");
            event_loop.exit();
            return;
        }
        if self.last_log.elapsed() >= Duration::from_secs(1) {
            self.last_log = Instant::now();
            tracing::info!(
                frames = renderer.frames_rendered(),
                display = ?renderer.size(),
                internal = ?renderer.internal_extent(renderer.size()),
                validation_messages = renderer.validation_message_count(),
                "tick"
            );
        }
        if self.limit.is_some_and(|l| self.start.elapsed() >= l) {
            event_loop.exit();
        }
    }
}

fn parse_quality(s: &str) -> Option<Quality> {
    Some(match s {
        "quality" => Quality::Quality,
        "balanced" => Quality::Balanced,
        "performance" => Quality::Performance,
        "ultra" => Quality::UltraPerformance,
        "native" => Quality::Native,
        _ => Quality::Free(s.strip_prefix("free:")?.parse().ok()?),
    })
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let kind = match value("--upscaler").unwrap_or("fsr1") {
        "native" => UpscalerKind::Native,
        "taa" => UpscalerKind::Taa,
        "fsr1" => UpscalerKind::Fsr1,
        "fsr3" => UpscalerKind::Fsr3,
        "dlss" => UpscalerKind::Dlss,
        other => {
            tracing::error!("unknown upscaler `{other}`, expected native, taa, fsr1, fsr3 or dlss");
            return;
        }
    };
    let Some(quality) = parse_quality(value("--quality").unwrap_or("balanced")) else {
        tracing::error!(
            "unknown quality, expected quality, balanced, performance, ultra, native or free:S"
        );
        return;
    };
    let seconds = value("--seconds")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cfg: RenderConfig {
            vsync: !args.iter().any(|a| a == "--no-vsync"),
            size: (1600, 900),
            ..RenderConfig::default()
        },
        kind,
        quality,
        rain: args.iter().any(|a| a == "--weather"),
        limit: (seconds > 0).then(|| Duration::from_secs(seconds)),
        start: Instant::now(),
        last_log: Instant::now(),
        meshes: None,
        prev_view_proj: None,
        renderer: None,
        window: None,
    };
    event_loop.run_app(&mut app).expect("event loop run");
    tracing::info!("exited cleanly");
}
