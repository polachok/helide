mod app;
mod backend;
mod config;
mod input;
mod renderer;

use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use helix_term::config::Config;
use helix_tui::backend::Backend;
use helix_view::input::Event;

use crate::app::HelideApp;
use crate::backend::GpuBackend;
use crate::renderer::Renderer;

#[derive(Debug, Clone)]
enum UserEvent {
    Redraw,
}

struct WinitApp {
    window: Option<Arc<Window>>,
    helide: Option<HelideApp>,
    cursor_position: (f64, f64),
    modifiers: winit::event::Modifiers,
    files: Vec<PathBuf>,
    proxy: EventLoopProxy<UserEvent>,
}

impl WinitApp {
    fn new(files: Vec<PathBuf>, proxy: EventLoopProxy<UserEvent>) -> Self {
        WinitApp {
            window: None,
            helide: None,
            cursor_position: (0.0, 0.0),
            modifiers: winit::event::Modifiers::default(),
            files,
            proxy,
        }
    }

    fn shutdown(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(mut helide) = self.helide.take() {
            // Graceful shutdown: flush writes, close LSP servers, finish jobs
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                if let Err(err) = helide
                    .jobs
                    .finish(&mut helide.editor, Some(&mut helide.compositor))
                    .await
                {
                    log::error!("Error finishing jobs: {err}");
                }
                if let Err(err) = helide.editor.flush_writes().await {
                    log::error!("Error flushing writes: {err}");
                }
                if helide.editor.close_language_servers(None).await.is_err() {
                    log::error!("Timed out waiting for language servers to shutdown");
                }
            });
        }
        self.window.take();
        event_loop.exit();
    }

    fn cell_size(helide: &HelideApp) -> (f32, f32) {
        let backend = helide.terminal.backend();
        (backend.cell_width(), backend.cell_height())
    }
}

impl ApplicationHandler<UserEvent> for WinitApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("helide")
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());

        // Create wgpu surface + device
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let (adapter, device, queue) = pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .expect("no suitable GPU adapter found");

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .expect("failed to create device");

            (adapter, device, queue)
        });

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // Prefer non-sRGB format to avoid double gamma correction
        // (our colors are already in sRGB space)
        let format = caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let helide_config = crate::config::HelideConfig::load();
        let scale_factor = window.scale_factor() as f32;
        let renderer = Renderer::new(
            device,
            queue,
            surface,
            config,
            &helide_config.font.family,
            helide_config.font.size * scale_factor,
        );
        let gpu_backend = GpuBackend::new(renderer);

        // Load helix config
        let editor_config = Config::load_default().unwrap_or_default();

        let files = std::mem::take(&mut self.files);
        let helide =
            HelideApp::new(gpu_backend, editor_config, files).expect("failed to init helide");

        self.helide = Some(helide);
        self.window = Some(window);

        // Spawn a periodic redraw timer for LSP spinners and async updates
        let proxy = self.proxy.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                if proxy.send_event(UserEvent::Redraw).is_err() {
                    break; // event loop closed
                }
            }
        });

        // Initial render
        self.helide.as_mut().unwrap().render();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: UserEvent) {
        if event_loop.exiting() {
            return;
        }
        if let Some(helide) = &mut self.helide {
            let needs_render = helide.poll_editor_events();
            if needs_render && !helide.editor.should_close() {
                helide.render();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // After exit is requested, ignore all events and drop resources
        if event_loop.exiting() {
            return;
        }

        let Some(helide) = &mut self.helide else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                self.shutdown(event_loop);
            }
            WindowEvent::Resized(size) => {
                helide
                    .terminal
                    .backend_mut()
                    .handle_resize(size.width, size.height);
                let backend_size = helide.terminal.backend().size().unwrap();
                helide.handle_resize(backend_size.width, backend_size.height);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let should_close = {
                    if let Some(hx_event) = input::convert_key_event(&event, &self.modifiers) {
                        helide.handle_event(hx_event);
                        helide.editor.should_close()
                    } else {
                        false
                    }
                };
                if should_close {
                    self.shutdown(event_loop);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let cell_size = Self::cell_size(helide);
                if let Some(hx_event) = input::convert_mouse_press(
                    state,
                    button,
                    self.cursor_position,
                    cell_size,
                    &self.modifiers,
                ) {
                    helide.handle_event(hx_event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let cell_size = Self::cell_size(helide);
                if let Some(hx_event) =
                    input::convert_scroll(delta, self.cursor_position, cell_size, &self.modifiers)
                {
                    helide.handle_event(hx_event);
                }
            }
            WindowEvent::Focused(focused) => {
                let event = if focused {
                    Event::FocusGained
                } else {
                    Event::FocusLost
                };
                helide.handle_event(event);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // Window size will change via a subsequent Resized event
                // Just request a redraw
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Poll async editor events (LSP, jobs, saves) before rendering
                helide.poll_editor_events();
                if !helide.editor.should_close() {
                    helide.render();
                    if let Some(window) = &self.window {
                        window.set_title(&helide.title());
                    }
                }
            }
            _ => {}
        }
    }
}

fn main() {
    // Set up tokio runtime — needed for helix async operations (LSP, jobs, word index)
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let _guard = runtime.enter();

    // Set HELIX_RUNTIME if not already set.
    // Search order: helix/runtime in cwd (dev), ~/.config/helix/runtime, next to `hx` binary
    if std::env::var("HELIX_RUNTIME").is_err() {
        let candidates: Vec<PathBuf> = [
            // Development: helix clone in cwd
            std::env::current_dir()
                .ok()
                .map(|d| d.join("helix/runtime")),
            // User config dir (where `hx` looks too)
            dirs::config_dir().map(|d| d.join("helix/runtime")),
            // Next to the installed `hx` binary (follows symlinks)
            which::which("hx")
                .ok()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .and_then(|p| p.parent().map(|d| d.join("runtime"))),
        ]
        .into_iter()
        .flatten()
        .collect();

        for candidate in candidates {
            if candidate.join("themes").exists() && candidate.join("queries").exists() {
                std::env::set_var("HELIX_RUNTIME", &candidate);
                break;
            }
        }
    }

    // Initialize helix runtime paths
    helix_loader::initialize_config_file(None);
    helix_loader::initialize_log_file(None);

    // Parse CLI args: helide [files...]
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();
    let proxy = event_loop.create_proxy();
    let mut app = WinitApp::new(files, proxy);
    event_loop.run_app(&mut app).unwrap();
}
