mod backend;
mod renderer;

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use helix_tui::buffer::Buffer;
use helix_view::graphics::{CursorKind, Style};

use crate::backend::GpuBackend;
use crate::renderer::Renderer;

struct App {
    window: Option<Arc<Window>>,
    backend: Option<GpuBackend>,
}

impl App {
    fn new() -> Self {
        App {
            window: None,
            backend: None,
        }
    }

    fn draw_demo(&mut self) {
        let backend = self.backend.as_mut().unwrap();
        let size = backend.size().unwrap();

        // Create a buffer and write demo content
        let mut buf = Buffer::empty(size);

        // Title bar
        let title_style = Style::default()
            .fg(helix_view::graphics::Color::Black)
            .bg(helix_view::graphics::Color::Rgb(100, 149, 237));
        buf.set_string(0, 0, " ".repeat(size.width as usize), title_style);
        buf.set_string(2, 0, " helide - GPU-accelerated Helix ", title_style);

        // Editor content
        let code_lines = [
            "fn main() {",
            "    let greeting = \"Hello from helide!\";",
            "    println!(\"{}\", greeting);",
            "}",
            "",
            "// This is rendered via wgpu + crossfont",
            "// No terminal emulator involved!",
        ];

        let normal = Style::default()
            .fg(helix_view::graphics::Color::Rgb(212, 212, 212))
            .bg(helix_view::graphics::Color::Rgb(30, 30, 30));

        let keyword_style = Style::default()
            .fg(helix_view::graphics::Color::Rgb(197, 134, 192))
            .bg(helix_view::graphics::Color::Rgb(30, 30, 30));

        let string_style = Style::default()
            .fg(helix_view::graphics::Color::Rgb(206, 145, 120))
            .bg(helix_view::graphics::Color::Rgb(30, 30, 30));

        let comment_style = Style::default()
            .fg(helix_view::graphics::Color::Rgb(106, 153, 85))
            .bg(helix_view::graphics::Color::Rgb(30, 30, 30));

        // Fill background
        for row in 1..size.height {
            buf.set_string(0, row, " ".repeat(size.width as usize), normal);
        }

        // Line numbers + code
        for (i, line) in code_lines.iter().enumerate() {
            let row = (i + 2) as u16;
            if row >= size.height {
                break;
            }
            let ln_style = Style::default()
                .fg(helix_view::graphics::Color::Rgb(133, 133, 133))
                .bg(helix_view::graphics::Color::Rgb(30, 30, 30));
            buf.set_string(0, row, format!(" {:>3} ", i + 1), ln_style);

            if line.starts_with("//") {
                buf.set_string(5, row, line, comment_style);
            } else if line.contains("fn ") || line.contains("let ") {
                // Simplified "syntax highlighting"
                buf.set_string(5, row, line, normal);
                // Highlight keywords
                if let Some(pos) = line.find("fn ") {
                    buf.set_string(5 + pos as u16, row, "fn", keyword_style);
                }
                if let Some(pos) = line.find("let ") {
                    buf.set_string(5 + pos as u16, row, "let", keyword_style);
                }
                // Highlight strings
                if let Some(start) = line.find('"') {
                    if let Some(end) = line[start + 1..].find('"') {
                        let s = &line[start..=start + 1 + end];
                        buf.set_string(5 + start as u16, row, s, string_style);
                    }
                }
            } else {
                buf.set_string(5, row, line, normal);
            }
        }

        // Status line
        let status_row = size.height - 1;
        let status_style = Style::default()
            .fg(helix_view::graphics::Color::Rgb(0, 0, 0))
            .bg(helix_view::graphics::Color::Rgb(0, 122, 204));
        buf.set_string(
            0,
            status_row,
            " ".repeat(size.width as usize),
            status_style,
        );
        buf.set_string(1, status_row, " NOR ", status_style);
        buf.set_string(7, status_row, " main.rs ", status_style);

        // Feed the buffer content into the backend via Backend::draw + flush
        use helix_tui::backend::Backend;
        let cells: Vec<(u16, u16, &helix_tui::buffer::Cell)> = buf
            .content()
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let x = (i % size.width as usize) as u16;
                let y = (i / size.width as usize) as u16;
                (x, y, cell)
            })
            .collect();

        backend.draw(cells.into_iter()).unwrap();
        backend.set_cursor(5, 2).unwrap();
        backend.show_cursor(CursorKind::Block).unwrap();
        backend.flush().unwrap();
    }
}

impl ApplicationHandler for App {
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
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
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
        let backend = GpuBackend::new(renderer);

        self.backend = Some(backend);
        self.window = Some(window);

        // Trigger initial draw
        self.draw_demo();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(backend) = &mut self.backend {
                    backend.handle_resize(size.width, size.height);
                    self.draw_demo();
                }
            }
            WindowEvent::RedrawRequested => {
                self.draw_demo();
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap();
}
