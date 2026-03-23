use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use helix_lsp::LspProgressMap;
use helix_term::compositor::{Compositor, Context, Event};
use helix_term::config::Config;
use helix_term::job::Jobs;
use helix_term::keymap::Keymaps;
use helix_term::ui;
use helix_tui::backend::Backend;
use helix_tui::terminal::Terminal;
use helix_view::graphics::Rect;
use helix_view::handlers::completion::CompletionHandler;
use helix_view::handlers::word_index;
use helix_view::handlers::Handlers;
use helix_view::theme;
use helix_view::Editor;

use crate::backend::GpuBackend;

pub struct HelideApp {
    pub compositor: Compositor,
    pub terminal: Terminal<GpuBackend>,
    pub editor: Editor,
    pub config: Arc<ArcSwap<Config>>,
    pub jobs: Jobs,
    pub lsp_progress: LspProgressMap,
}

impl HelideApp {
    pub fn new(backend: GpuBackend, config: Config, files: Vec<PathBuf>) -> anyhow::Result<Self> {
        // Register helix-term events (required before creating handlers)
        helix_term::events::register();

        let area = backend.size()?;
        let mut terminal = Terminal::new(backend)?;

        // Theme loader
        let mut theme_parent_dirs = vec![helix_loader::config_dir()];
        theme_parent_dirs.extend(helix_loader::runtime_dirs().iter().cloned());
        let theme_loader = theme::Loader::new(&theme_parent_dirs);

        let config = Arc::new(ArcSwap::from_pointee(config));

        // Create handlers with dummy channels for LSP features.
        // word_index is real since it's self-contained.
        let handlers = create_handlers();

        let lang_loader = helix_core::config::user_lang_loader()
            .unwrap_or_else(|_| helix_core::config::default_lang_loader());

        let mut editor = Editor::new(
            area,
            Arc::new(theme_loader),
            Arc::new(ArcSwap::from_pointee(lang_loader)),
            Arc::new(arc_swap::access::Map::new(
                Arc::clone(&config),
                |config: &Config| &config.editor,
            )),
            handlers,
        );

        // Load theme
        let true_color = true; // GPU backend always supports true color
        let theme_name = {
            let cfg = config.load();
            cfg.theme
                .as_ref()
                .map(|t| t.choose(None).to_string())
                .unwrap_or_else(|| "default".to_string())
        };
        let theme = editor
            .theme_loader
            .load(&theme_name)
            .unwrap_or_else(|_| editor.theme_loader.default_theme(true_color));
        let _ = editor.set_theme(theme);

        // Extract theme default colors for the renderer
        update_renderer_theme(&editor.theme, terminal.backend_mut());

        // Set up compositor with editor view
        let mut compositor = Compositor::new(area);
        let keys = Box::new(arc_swap::access::Map::new(
            Arc::clone(&config),
            |config: &Config| &config.keys,
        ));
        let editor_view = Box::new(ui::EditorView::new(Keymaps::new(keys)));
        compositor.push(editor_view);

        let jobs = Jobs::new();

        // Open files or scratch buffer
        if files.is_empty() {
            editor.new_file(helix_view::editor::Action::VerticalSplit);
        } else {
            let action = helix_view::editor::Action::VerticalSplit;
            for file in &files {
                if let Err(e) = editor.open(file, action) {
                    log::error!("Failed to open {}: {}", file.display(), e);
                }
            }
            if editor.documents().next().is_none() {
                editor.new_file(action);
            }
        }

        Ok(HelideApp {
            compositor,
            terminal,
            editor,
            config,
            jobs,
            lsp_progress: LspProgressMap::new(),
        })
    }

    /// Handle a helix Event (key, mouse, resize, etc.)
    pub fn handle_event(&mut self, event: Event) -> bool {
        let mut cx = Context {
            editor: &mut self.editor,
            jobs: &mut self.jobs,
            scroll: None,
        };

        let consumed = self.compositor.handle_event(&event, &mut cx);

        // Don't render if the editor is closing — the view tree may be empty
        if self.editor.should_close() {
            return consumed;
        }

        if consumed || self.editor.needs_redraw {
            self.render();
            true
        } else {
            false
        }
    }

    /// Handle resize
    pub fn handle_resize(&mut self, cols: u16, rows: u16) {
        let area = Rect::new(0, 0, cols, rows);
        self.terminal
            .resize(area)
            .expect("failed to resize terminal");
        self.compositor.resize(area);
        self.render();
    }

    /// Render the editor state to the GPU.
    pub fn render(&mut self) {
        // GPU rendering is cheap — always do a full redraw
        let _ = self.terminal.clear();

        let mut cx = Context {
            editor: &mut self.editor,
            jobs: &mut self.jobs,
            scroll: None,
        };

        helix_event::start_frame();
        cx.editor.needs_redraw = false;

        let area = self
            .terminal
            .autoresize()
            .expect("unable to determine terminal size");

        let surface = self.terminal.current_buffer_mut();
        self.compositor.render(area, surface, &mut cx);
        let (pos, kind) = self.compositor.cursor(area, &self.editor);
        self.editor.cursor_cache.reset();

        let pos = pos.map(|pos| (pos.col as u16, pos.row as u16));
        self.terminal.draw(pos, kind).unwrap();
    }

    /// Poll editor async events (LSP responses, jobs, etc.)
    /// Call this periodically from the event loop.
    pub fn poll_editor_events(&mut self) {
        use futures_util::Stream;
        use std::task::{Context as TaskContext, Poll};

        // Process pending job callbacks
        while let Ok(callback) = self.jobs.callbacks.try_recv() {
            self.jobs
                .handle_callback(&mut self.editor, &mut self.compositor, Ok(Some(callback)));
        }

        // Process status messages
        while let Ok(msg) = self.jobs.status_messages.try_recv() {
            use helix_core::diagnostic::Severity;
            let severity = match msg.severity {
                helix_event::status::Severity::Hint => Severity::Hint,
                helix_event::status::Severity::Info => Severity::Info,
                helix_event::status::Severity::Warning => Severity::Warning,
                helix_event::status::Severity::Error => Severity::Error,
            };
            self.editor.status_msg = Some((msg.message, severity));
        }

        // Poll wait_futures (FuturesUnordered stream)
        let waker = futures_util::task::noop_waker();
        let mut cx = TaskContext::from_waker(&waker);
        while let Poll::Ready(Some(callback)) =
            std::pin::Pin::new(&mut self.jobs.wait_futures).poll_next(&mut cx)
        {
            self.jobs
                .handle_callback(&mut self.editor, &mut self.compositor, callback);
        }
    }
}

/// Create Handlers with dummy channels for LSP features.
/// This allows the editor to work for basic editing without full LSP support.
fn create_handlers() -> Handlers {
    let (completion_tx, _) = tokio::sync::mpsc::channel(1);
    let (sig_tx, _) = tokio::sync::mpsc::channel(1);
    let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_links_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_diag_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_all_diag_tx, _) = tokio::sync::mpsc::channel(1);

    Handlers {
        completions: CompletionHandler::new(completion_tx),
        signature_hints: sig_tx,
        auto_save: auto_save_tx,
        document_colors: doc_colors_tx,
        document_links: doc_links_tx,
        word_index: word_index::Handler::spawn(),
        pull_diagnostics: pull_diag_tx,
        pull_all_documents_diagnostics: pull_all_diag_tx,
    }
}

/// Extract default fg/bg colors from the helix theme and apply to the GPU backend.
fn update_renderer_theme(theme: &helix_view::Theme, backend: &mut crate::backend::GpuBackend) {
    use helix_view::graphics::Color;

    let bg_style = theme.get("ui.background");
    let fg_style = theme.get("ui.text");

    let default_fg = match fg_style.fg {
        Some(Color::Rgb(r, g, b)) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
        _ => [0.85, 0.85, 0.85, 1.0],
    };

    let default_bg = match bg_style.bg {
        Some(Color::Rgb(r, g, b)) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
        _ => [0.1, 0.1, 0.1, 1.0],
    };

    backend.set_default_colors(default_fg, default_bg);
}
