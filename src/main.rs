mod app;
mod backend;
mod input;
mod renderer;

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use helix_term::config::Config;
use helix_tui::backend::Backend;
use helix_view::input::Event;

use crate::app::HelideApp;
use crate::backend::GpuBackend;
use crate::renderer::Renderer;

struct WinitApp {
    window: Option<Arc<Window>>,
    helide: Option<HelideApp>,
    cursor_position: (f64, f64),
    modifiers: winit::event::Modifiers,
}

impl WinitApp {
    fn new() -> Self {
        WinitApp {
            window: None,
            helide: None,
            cursor_position: (0.0, 0.0),
            modifiers: winit::event::Modifiers::default(),
        }
    }

    fn shutdown(&mut self, event_loop: &ActiveEventLoop) {
        // Drop helide app (and all wgpu resources) before exiting the event loop
        self.helide.take();
        self.window.take();
        event_loop.exit();
    }

    fn cell_size(helide: &HelideApp) -> (f32, f32) {
        let backend = helide.terminal.backend();
        (backend.cell_width(), backend.cell_height())
    }
}

impl ApplicationHandler for WinitApp {
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

        let scale_factor = window.scale_factor() as f32;
        let renderer = Renderer::new(device, queue, surface, config, 16.0 * scale_factor);
        let gpu_backend = GpuBackend::new(renderer);

        // Load helix config
        let editor_config = Config::load_default().unwrap_or_default();

        let helide = HelideApp::new(gpu_backend, editor_config).expect("failed to init helide");

        self.helide = Some(helide);
        self.window = Some(window);

        // Initial render
        self.helide.as_mut().unwrap().render();
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
            WindowEvent::RedrawRequested => {
                // Poll async editor events before rendering
                helide.poll_editor_events();
                helide.render();
            }
            _ => {}
        }
    }
}

fn main() {
    // Set up tokio runtime — needed for helix async operations (LSP, jobs, word index)
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let _guard = runtime.enter();

    // Initialize helix runtime paths
    helix_loader::initialize_config_file(None);
    helix_loader::initialize_log_file(None);

    let event_loop = EventLoop::new().unwrap();
    let mut app = WinitApp::new();
    event_loop.run_app(&mut app).unwrap();
}
