//! Manual check of the windowed frame loop: a grid of cubes seen by a slowly
//! orbiting camera (jittered, reverse-Z). Resize it and minimise it; the log shows the frame
//! count, which stops growing while minimised.
//!
//! `window [--no-vsync] [--seconds N]` (N = 0 runs until closed).

use std::time::{Duration, Instant};

use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    cube_mesh, jitter, perspective_reverse_z, Camera, Instance, MeshId, RenderConfig, RenderWorld,
    Renderer, CUBE_FACE_TINTS,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    cfg: RenderConfig,
    limit: Option<Duration>,
    start: Instant,
    last_log: Instant,
    cube: Option<MeshId>,
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
            .with_title("Faset render check")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.cfg.size.0,
                self.cfg.size.1,
            ));
        let window = event_loop.create_window(attrs).expect("create window");
        match Renderer::new_windowed(&window, &self.cfg) {
            Ok(mut r) => {
                let (v, i) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
                match r.upload_mesh(&v, &i) {
                    Ok(id) => self.cube = Some(id),
                    Err(e) => tracing::error!("mesh upload failed: {e}"),
                }
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
                tracing::info!(width = size.width, height = size.height, "resized");
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
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let t = self.start.elapsed().as_secs_f32();
        let (w, h) = renderer.size();
        let eye = Vec3::new(12.0 * (0.2 * t).cos(), 7.0, 12.0 * (0.2 * t).sin());
        let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
        let proj =
            perspective_reverse_z(60f32.to_radians(), w.max(1) as f32 / h.max(1) as f32, 0.1);
        let mut camera = Camera::new(view, proj);
        if let Some(prev) = self.prev_view_proj {
            camera.prev_view_proj = prev;
        }
        camera.jitter_px = jitter(renderer.frames_rendered() as u32);
        self.prev_view_proj = Some(camera.view_proj());
        let instances = self
            .cube
            .map(|cube| {
                (0..100)
                    .map(|k| {
                        let pos = Vec3::new(
                            (k % 10) as f32 * 1.5 - 6.75,
                            0.0,
                            (k / 10) as f32 * 1.5 - 6.75,
                        );
                        Instance::new(
                            cube,
                            0,
                            Mat4::from_translation(pos) * Mat4::from_rotation_y(0.1 * k as f32),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let world = RenderWorld {
            clear_color: [0.05, 0.07, 0.1, 1.0],
            camera,
            instances,
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
                size = ?renderer.size(),
                validation_messages = renderer.validation_message_count(),
                "tick"
            );
        }
        if self.limit.is_some_and(|l| self.start.elapsed() >= l) {
            event_loop.exit();
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vsync = !args.iter().any(|a| a == "--no-vsync");
    let seconds = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cfg: RenderConfig {
            vsync,
            ..RenderConfig::default()
        },
        limit: (seconds > 0).then(|| Duration::from_secs(seconds)),
        start: Instant::now(),
        last_log: Instant::now(),
        cube: None,
        prev_view_proj: None,
        renderer: None,
        window: None,
    };
    event_loop.run_app(&mut app).expect("event loop run");
    tracing::info!("exited cleanly");
}
