mod app;
mod backend;
mod config;
mod input;
mod layout;
mod platform;
mod renderer;
mod terminal;

use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use helix_term::config::Config;
use helix_tui::backend::Backend;
use helix_view::input::Event;

use crate::app::HelideApp;
use crate::backend::GpuBackend;
use crate::renderer::Renderer;

#[derive(Debug, Clone)]
pub enum UserEvent {
    Redraw,
    NewFile,
    OpenFile(PathBuf),
    OpenDirectory(PathBuf),
    Save,
    CloseBuffer,
    Undo,
    Redo,
    Paste,
    Tutor,
}

struct WinitApp {
    window: Option<Arc<Window>>,
    helide: Option<HelideApp>,
    cursor_position: (f64, f64),
    modifiers: winit::event::Modifiers,
    files: Vec<PathBuf>,
    proxy: EventLoopProxy<UserEvent>,
    scroll: input::ScrollAccumulator,
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
            scroll: input::ScrollAccumulator::new(),
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

        let mut attrs = Window::default_attributes()
            .with_title("helide")
            .with_inner_size(winit::dpi::LogicalSize::new(1200.0, 800.0));

        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true);
        }

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
        // Non-sRGB format — colors are passed through as-is in sRGB space.
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
        let mut renderer = Renderer::new(
            device,
            queue,
            surface,
            config,
            &helide_config.font.family,
            helide_config.font.size * scale_factor,
        );

        // On macOS with transparent titlebar, add top padding for the titlebar
        #[cfg(target_os = "macos")]
        {
            let titlebar_height = 28.0 * scale_factor; // standard macOS titlebar
            renderer.set_padding_top(titlebar_height);
        }

        let gpu_backend = GpuBackend::new(renderer);

        // Load helix config
        let editor_config = Config::load_default().unwrap_or_default();

        let files = std::mem::take(&mut self.files);
        let helide =
            HelideApp::new(gpu_backend, editor_config, files, self.proxy.clone()).expect("failed to init helide");

        // Set up native macOS menu bar
        #[cfg(target_os = "macos")]
        platform::macos::setup_menu_bar();

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

        // Flush any files received via Apple Event during startup
        #[cfg(target_os = "macos")]
        platform::macos::flush_pending_files();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if event_loop.exiting() {
            return;
        }
        match event {
            UserEvent::Redraw => {
                // Flush any pending Apple Event file opens
                #[cfg(target_os = "macos")]
                platform::macos::flush_pending_files();

                if let Some(helide) = &mut self.helide {
                    let mut needs_render = helide.poll_editor_events();
                    // Poll terminal events
                    if let Some(pane) = &mut helide.terminal_pane {
                        pane.poll_events();
                        if pane.dirty {
                            needs_render = true;
                        }
                    }
                    if needs_render && !helide.editor.should_close() {
                        helide.render();
                    }
                }
            }
            UserEvent::NewFile => {
                if let Some(helide) = &mut self.helide {
                    helide
                        .editor
                        .new_file(helix_view::editor::Action::VerticalSplit);
                    helide.render();
                }
            }
            UserEvent::OpenFile(path) => {
                if let Some(helide) = &mut self.helide {
                    if let Err(e) = helide
                        .editor
                        .open(&path, helix_view::editor::Action::VerticalSplit)
                    {
                        helide.editor.set_error(format!("Failed to open: {e}"));
                    } else {
                        #[cfg(target_os = "macos")]
                        platform::macos::note_recent_document(&path);
                    }
                    helide.render();
                }
            }
            UserEvent::OpenDirectory(path) => {
                if let Some(helide) = &mut self.helide {
                    if let Err(e) = helix_stdx::env::set_current_working_dir(&path) {
                        helide
                            .editor
                            .set_error(format!("Failed to change directory: {e}"));
                    } else {
                        let has_envrc = path.join(".envrc").exists();
                        helide
                            .editor
                            .set_status(format!("Changed directory to {}", path.display()));
                        if has_envrc {
                            helide.editor.set_status("Loading direnv...");
                            helide.render();
                            load_direnv_with_status(&path, Some(&mut helide.editor));
                        }
                    }
                    helide.render();
                }
            }
            UserEvent::Save => {
                if let Some(helide) = &mut self.helide {
                    let doc_id = helix_view::doc!(helide.editor).id();
                    let _ = helide.editor.save::<PathBuf>(doc_id, None, false);
                    helide.render();
                }
            }
            UserEvent::Undo => {
                if let Some(helide) = &mut self.helide {
                    let (view, doc) = helix_view::current!(helide.editor);
                    doc.undo(view);
                    helide.render();
                }
            }
            UserEvent::Redo => {
                if let Some(helide) = &mut self.helide {
                    let (view, doc) = helix_view::current!(helide.editor);
                    doc.redo(view);
                    helide.render();
                }
            }
            UserEvent::Paste => {
                // Event::Paste is helix's proper API for OS-level paste
                if let Some(helide) = &mut self.helide {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            if !text.is_empty() {
                                helide.handle_event(Event::Paste(text));
                            }
                        }
                    }
                }
            }
            UserEvent::Tutor => {
                if let Some(helide) = &mut self.helide {
                    let path = helix_loader::runtime_file(std::path::Path::new("tutor"));
                    if let Err(e) = helide
                        .editor
                        .open(&path, helix_view::editor::Action::Replace)
                    {
                        helide
                            .editor
                            .set_error(format!("Failed to open tutor: {e}"));
                    } else {
                        helix_view::doc_mut!(helide.editor).set_path(None);
                    }
                    helide.render();
                }
            }
            UserEvent::CloseBuffer => {
                if let Some(helide) = &mut self.helide {
                    let doc_id = helix_view::doc!(helide.editor).id();
                    if let Err(_) = helide.editor.close_document(doc_id, false) {
                        helide
                            .editor
                            .set_error("Buffer has unsaved changes".to_string());
                    }
                    if !helide.editor.should_close() {
                        helide.render();
                    } else {
                        self.shutdown(event_loop);
                    }
                }
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
                // macOS behavior: hide window if no unsaved changes, keep app in dock
                #[cfg(target_os = "macos")]
                {
                    let has_unsaved = helide.editor.documents().any(|doc| doc.is_modified());
                    if has_unsaved {
                        helide
                            .editor
                            .set_error("Unsaved changes. Use :w to save or :q! to force quit.");
                        helide.render();
                    } else if let Some(window) = &self.window {
                        window.set_visible(false);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                self.shutdown(event_loop);
            }
            WindowEvent::Resized(size) => {
                // Resize swapchain surface to full window size first
                helide.terminal.backend_mut().handle_window_resize(size.width, size.height);
                // Then resize regions
                helide.layout.set_window_size(size.width, size.height);
                let regions = helide.layout.regions();
                let (_, _, ew, eh) = regions.editor;
                helide.terminal.backend_mut().handle_resize(ew, eh);
                let backend_size = helide.terminal.backend().size().unwrap();
                helide.handle_resize(backend_size.width, backend_size.height);
                if let (Some(pane), Some((_, _, tw, th))) = (&mut helide.terminal_pane, regions.terminal) {
                    pane.resize(helide.terminal.backend().renderer(), tw, th);
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Global: toggle terminal
                if input::is_toggle_terminal(&event, &self.modifiers) {
                    helide.toggle_terminal();
                    return;
                }

                match helide.focus {
                    app::Focus::Editor => {
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
                    app::Focus::Terminal => {
                        if let Some(pane) = &helide.terminal_pane {
                            if let Some(bytes) = crate::terminal::input::encode_key(&event, &self.modifiers) {
                                pane.write_to_pty(&bytes);
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (cx, cy) = self.cursor_position;

                // Divider drag
                if state == ElementState::Pressed && button == winit::event::MouseButton::Left {
                    if helide.layout.hit_test_divider(cx as f32, cy as f32) {
                        helide.layout.drag_start(cy as f32);
                        return;
                    }
                }
                if state == ElementState::Released && helide.layout.is_dragging() {
                    helide.layout.drag_end();
                    helide.render();
                    return;
                }

                // Focus + event routing
                use crate::layout::RegionKind;
                let new_focus = match helide.layout.region_at(cx as f32, cy as f32) {
                    RegionKind::Editor => app::Focus::Editor,
                    RegionKind::Terminal => app::Focus::Terminal,
                    RegionKind::Divider => helide.focus,
                };
                let focus_changed = new_focus != helide.focus;
                helide.focus = new_focus;

                match new_focus {
                    app::Focus::Editor => {
                        let cell_size = Self::cell_size(helide);
                        if let Some(hx_event) = input::convert_mouse_press(
                            state, button, self.cursor_position, cell_size, &self.modifiers,
                        ) {
                            helide.handle_event(hx_event);
                        }
                    }
                    app::Focus::Terminal => {}
                }

                if focus_changed {
                    if let Some(pane) = &mut helide.terminal_pane {
                        pane.dirty = true;
                    }
                    helide.render();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
                if helide.layout.is_dragging() {
                    if helide.layout.drag_update(position.y as f32) {
                        let regions = helide.layout.regions();
                        let (_, _, ew, eh) = regions.editor;
                        helide.terminal.backend_mut().handle_resize(ew, eh);
                        let backend_size = helide.terminal.backend().size().unwrap();
                        helide.handle_resize(backend_size.width, backend_size.height);
                        if let (Some(pane), Some((_, _, tw, th))) = (&mut helide.terminal_pane, regions.terminal) {
                            pane.resize(helide.terminal.backend().renderer(), tw, th);
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let cell_size = Self::cell_size(helide);
                let events =
                    self.scroll
                        .accumulate(delta, self.cursor_position, cell_size, &self.modifiers);
                for hx_event in events {
                    helide.handle_event(hx_event);
                }
            }
            WindowEvent::DroppedFile(path) => {
                let should_open = path.is_file()
                    && std::fs::metadata(&path)
                        .map(|m| m.len() < 32 * 1024 * 1024) // skip files > 32MB
                        .unwrap_or(false)
                    && std::fs::read(&path)
                        .map(|bytes| {
                            let sample = &bytes[..bytes.len().min(8192)];
                            content_inspector::inspect(sample).is_text()
                        })
                        .unwrap_or(false);

                if should_open {
                    if let Err(e) = helide
                        .editor
                        .open(&path, helix_view::editor::Action::VerticalSplit)
                    {
                        helide.editor.set_error(format!("Failed to open: {e}"));
                    }
                    helide.render();
                } else {
                    helide
                        .editor
                        .set_error(format!("Cannot open: {}", path.display()));
                    helide.render();
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
                if let Some(pane) = &mut helide.terminal_pane {
                    pane.poll_events();
                }
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

/// Inherit PATH from the user's login shell.
/// macOS GUI apps get a minimal PATH; this gives us the real one.
fn inherit_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    if let Ok(output) = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo $PATH"])
        .output()
    {
        if let Ok(path) = String::from_utf8(output.stdout) {
            let path = path.trim();
            if !path.is_empty() {
                std::env::set_var("PATH", path);
            }
        }
    }
}

/// Load direnv environment variables for the given directory.
fn load_direnv(dir: &std::path::Path) {
    let Ok(output) = std::process::Command::new("direnv")
        .args(["export", "json"])
        .current_dir(dir)
        .output()
    else {
        return;
    };

    if !output.status.success() {
        return;
    }

    let Ok(vars) =
        serde_json::from_slice::<std::collections::HashMap<String, String>>(&output.stdout)
    else {
        return;
    };

    let count = vars.len();
    for (key, value) in vars {
        std::env::set_var(&key, &value);
    }
    log::info!("direnv: loaded {count} env vars");
}

/// Load direnv and report to the editor status line.
fn load_direnv_with_status(dir: &std::path::Path, editor: Option<&mut helix_view::Editor>) {
    if !dir.join(".envrc").exists() {
        return;
    }

    let Ok(output) = std::process::Command::new("direnv")
        .args(["export", "json"])
        .current_dir(dir)
        .output()
    else {
        if let Some(editor) = editor {
            editor.set_status("direnv: not installed");
        }
        return;
    };

    if !output.status.success() {
        if let Some(editor) = editor {
            editor.set_error("direnv: failed to export environment");
        }
        return;
    }

    let Ok(vars) =
        serde_json::from_slice::<std::collections::HashMap<String, String>>(&output.stdout)
    else {
        return;
    };

    let count = vars.len();
    for (key, value) in &vars {
        std::env::set_var(key, value);
    }
    if let Some(editor) = editor {
        editor.set_status(format!("direnv: loaded {count} env vars"));
    }
}

fn main() {
    // Inherit the user's login shell PATH (macOS GUI apps get minimal PATH)
    inherit_shell_path();

    // Load direnv if .envrc exists in cwd
    if std::path::Path::new(".envrc").exists() {
        load_direnv(&std::env::current_dir().unwrap_or_default());
    }

    // Set up tokio runtime — needed for helix async operations (LSP, jobs, word index)
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let _guard = runtime.enter();

    // Set HELIX_RUNTIME if not already set.
    // Search order: helix/runtime in cwd (dev), ~/.config/helix/runtime, next to `hx` binary
    if std::env::var("HELIX_RUNTIME").is_err() {
        let candidates: Vec<PathBuf> = [
            // macOS app bundle: Contents/Resources/runtime
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent()?.parent().map(|d| d.join("Resources/runtime"))),
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

    // Set up macOS: proxy early for queuing, then register handler (delegate exists after build())
    #[cfg(target_os = "macos")]
    {
        platform::macos::set_event_proxy(proxy.clone());
        platform::macos::register_open_file_handler();
    }

    let mut app = WinitApp::new(files, proxy);
    event_loop.run_app(&mut app).unwrap();
}
