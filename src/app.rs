use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use helix_lsp::{lsp, lsp::notification::Notification as _, LanguageServerId, LspProgressMap};
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

    /// Returns the title string based on the focused document.
    pub fn title(&self) -> String {
        let (_view, doc) = helix_view::current_ref!(&self.editor);
        let name = doc
            .path()
            .map(|p| {
                helix_stdx::path::get_relative_path(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "[scratch]".into());
        let modified = if doc.is_modified() { " [+]" } else { "" };
        format!("{name}{modified} - helide")
    }

    /// Poll editor async events (LSP, jobs, saves) using tokio.
    /// Returns true if a redraw is needed.
    pub fn poll_editor_events(&mut self) -> bool {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(self.poll_editor_events_async())
    }

    async fn poll_editor_events_async(&mut self) -> bool {
        use futures_util::StreamExt;
        use tokio::time::{timeout, Duration};

        let mut needs_render = false;

        // Process pending job callbacks (sync channel)
        while let Ok(callback) = self.jobs.callbacks.try_recv() {
            self.jobs
                .handle_callback(&mut self.editor, &mut self.compositor, Ok(Some(callback)));
            needs_render = true;
        }

        // Process status messages (sync channel)
        while let Ok(msg) = self.jobs.status_messages.try_recv() {
            use helix_core::diagnostic::Severity;
            let severity = match msg.severity {
                helix_event::status::Severity::Hint => Severity::Hint,
                helix_event::status::Severity::Info => Severity::Info,
                helix_event::status::Severity::Warning => Severity::Warning,
                helix_event::status::Severity::Error => Severity::Error,
            };
            self.editor.status_msg = Some((msg.message, severity));
            needs_render = true;
        }

        // Drain all ready async events with a short timeout
        let deadline = Duration::from_millis(5);
        let _ = timeout(deadline, async {
            loop {
                tokio::select! {
                    biased;

                    Some(callback) = self.jobs.wait_futures.next() => {
                        self.jobs.handle_callback(
                            &mut self.editor,
                            &mut self.compositor,
                            callback,
                        );
                        needs_render = true;
                    }

                    Some((server_id, call)) = self.editor.language_servers.incoming.next() => {
                        self.handle_language_server_message(call, server_id).await;
                        needs_render = true;
                    }

                    Some(event) = self.editor.save_queue.next() => {
                        self.editor.write_count -= 1;
                        self.handle_document_write(event);
                        needs_render = true;
                    }

                    else => break,
                }
            }
        })
        .await;

        needs_render
    }

    fn handle_document_write(
        &mut self,
        doc_save_event: helix_view::document::DocumentSavedEventResult,
    ) {
        let doc_save_event = match doc_save_event {
            Ok(event) => event,
            Err(err) => {
                self.editor.set_error(err.to_string());
                return;
            }
        };

        let doc = match self.editor.document_mut(doc_save_event.doc_id) {
            Some(doc) => doc,
            None => return,
        };

        doc.set_last_saved_revision(doc_save_event.revision, doc_save_event.save_time);

        let lines = doc_save_event.text.len_lines();
        let bytes = doc_save_event.text.len_bytes();
        let size_str = if bytes < 1024 {
            format!("{bytes}B")
        } else {
            let mut size = bytes as f32;
            let suffixes = ["B", "KiB", "MiB", "GiB"];
            let mut i = 0;
            while i < suffixes.len() - 1 && size >= 1024.0 {
                size /= 1024.0;
                i += 1;
            }
            format!("{size:.1}{}", suffixes[i])
        };

        self.editor
            .set_doc_path(doc_save_event.doc_id, &doc_save_event.path);
        self.editor.set_status(format!(
            "'{}' written, {lines}L {size_str}",
            helix_stdx::path::get_relative_path(&doc_save_event.path).to_string_lossy(),
        ));
    }

    fn handle_show_message(&mut self, typ: lsp::MessageType, message: String) {
        if self.config.load().editor.lsp.display_messages {
            match typ {
                lsp::MessageType::ERROR => self.editor.set_error(message),
                lsp::MessageType::WARNING => self.editor.set_warning(message),
                _ => self.editor.set_status(message),
            }
        }
    }

    /// Ported from helix-term Application::handle_language_server_message
    async fn handle_language_server_message(
        &mut self,
        call: helix_lsp::Call,
        server_id: LanguageServerId,
    ) {
        use helix_lsp::{Call, MethodCall, Notification};

        macro_rules! language_server {
            () => {
                match self.editor.language_server_by_id(server_id) {
                    Some(language_server) => language_server,
                    None => {
                        log::warn!("can't find language server with id `{}`", server_id);
                        return;
                    }
                }
            };
        }

        match call {
            Call::Notification(helix_lsp::jsonrpc::Notification { method, params, .. }) => {
                let notification = match Notification::parse(&method, params) {
                    Ok(notification) => notification,
                    Err(helix_lsp::Error::Unhandled) => {
                        log::info!("Ignoring unhandled LSP notification");
                        return;
                    }
                    Err(err) => {
                        log::error!("Ignoring unknown LSP notification: {}", err);
                        return;
                    }
                };

                match notification {
                    Notification::Initialized => {
                        let language_server = language_server!();
                        if let Some(config) = language_server.config() {
                            language_server.did_change_configuration(config.clone());
                        }
                        helix_event::dispatch(helix_view::events::LanguageServerInitialized {
                            editor: &mut self.editor,
                            server_id,
                        });
                    }
                    Notification::PublishDiagnostics(params) => {
                        let uri = match helix_core::Uri::try_from(params.uri) {
                            Ok(uri) => uri,
                            Err(err) => {
                                log::error!("{err}");
                                return;
                            }
                        };
                        let language_server = language_server!();
                        if !language_server.is_initialized() {
                            log::error!(
                                "Discarding publishDiagnostic from uninitialized server: {}",
                                language_server.name()
                            );
                            return;
                        }
                        let provider = helix_core::diagnostic::DiagnosticProvider::Lsp {
                            server_id,
                            identifier: None,
                        };
                        self.editor.handle_lsp_diagnostics(
                            &provider,
                            uri,
                            params.version,
                            params.diagnostics,
                        );
                    }
                    Notification::ShowMessage(params) => {
                        self.handle_show_message(params.typ, params.message);
                    }
                    Notification::LogMessage(params) => {
                        log::info!("window/logMessage: {:?}", params);
                    }
                    Notification::ProgressMessage(params)
                        if !self
                            .compositor
                            .has_component(std::any::type_name::<ui::Prompt>()) =>
                    {
                        let lsp::ProgressParams {
                            token,
                            value: lsp::ProgressParamsValue::WorkDone(work),
                        } = params;

                        let editor_view = self
                            .compositor
                            .find::<ui::EditorView>()
                            .expect("expected EditorView");

                        let (title, message, percentage) = match &work {
                            lsp::WorkDoneProgress::Begin(b) => {
                                (Some(b.title.as_str()), &b.message, &b.percentage)
                            }
                            lsp::WorkDoneProgress::Report(r) => (None, &r.message, &r.percentage),
                            lsp::WorkDoneProgress::End(lsp::WorkDoneProgressEnd { message }) => {
                                if message.is_some() {
                                    (None, message, &None)
                                } else {
                                    self.lsp_progress.end_progress(server_id, &token);
                                    if !self.lsp_progress.is_progressing(server_id) {
                                        editor_view.spinners_mut().get_or_create(server_id).stop();
                                    }
                                    self.editor.clear_status();
                                    return;
                                }
                            }
                        };

                        if self.editor.config().lsp.display_progress_messages {
                            let title = title.or_else(|| {
                                self.lsp_progress
                                    .title(server_id, &token)
                                    .map(|s| s.as_str())
                            });
                            if title.is_some() || percentage.is_some() || message.is_some() {
                                use std::fmt::Write as _;
                                let mut status = format!("{}: ", language_server!().name());
                                if let Some(pct) = percentage {
                                    write!(status, "{pct:>2}% ").unwrap();
                                }
                                if let Some(title) = title {
                                    status.push_str(title);
                                }
                                if title.is_some() && message.is_some() {
                                    status.push_str(" ⋅ ");
                                }
                                if let Some(msg) = message {
                                    status.push_str(msg);
                                }
                                self.editor.set_status(status);
                            }
                        }

                        match work {
                            lsp::WorkDoneProgress::Begin(begin) => {
                                self.lsp_progress.begin(server_id, token.clone(), begin);
                            }
                            lsp::WorkDoneProgress::Report(report) => {
                                self.lsp_progress.update(server_id, token.clone(), report);
                            }
                            lsp::WorkDoneProgress::End(_) => {
                                self.lsp_progress.end_progress(server_id, &token);
                                if !self.lsp_progress.is_progressing(server_id) {
                                    editor_view.spinners_mut().get_or_create(server_id).stop();
                                }
                            }
                        }
                    }
                    Notification::ProgressMessage(_) => {}
                    Notification::Exit => {
                        self.editor.set_status("Language server exited");

                        for diags in self.editor.diagnostics.values_mut() {
                            diags.retain(|(_, provider)| {
                                provider.language_server_id() != Some(server_id)
                            });
                        }
                        self.editor.diagnostics.retain(|_, diags| !diags.is_empty());

                        for doc in self.editor.documents_mut() {
                            doc.clear_diagnostics_for_language_server(server_id);
                        }

                        helix_event::dispatch(helix_view::events::LanguageServerExited {
                            editor: &mut self.editor,
                            server_id,
                        });

                        self.editor.language_servers.remove_by_id(server_id);
                    }
                }
            }
            Call::MethodCall(helix_lsp::jsonrpc::MethodCall {
                method, params, id, ..
            }) => {
                let reply = match MethodCall::parse(&method, params) {
                    Err(helix_lsp::Error::Unhandled) => {
                        log::error!("Language Server: Method {method} not found in request {id}");
                        Err(helix_lsp::jsonrpc::Error {
                            code: helix_lsp::jsonrpc::ErrorCode::MethodNotFound,
                            message: format!("Method not found: {method}"),
                            data: None,
                        })
                    }
                    Err(err) => {
                        log::error!("Language Server: Malformed method call {method}: {err}");
                        Err(helix_lsp::jsonrpc::Error {
                            code: helix_lsp::jsonrpc::ErrorCode::ParseError,
                            message: format!("Malformed method call: {method}"),
                            data: None,
                        })
                    }
                    Ok(MethodCall::WorkDoneProgressCreate(params)) => {
                        self.lsp_progress.create(server_id, params.token);
                        let editor_view = self
                            .compositor
                            .find::<ui::EditorView>()
                            .expect("expected EditorView");
                        let spinner = editor_view.spinners_mut().get_or_create(server_id);
                        if spinner.is_stopped() {
                            spinner.start();
                        }
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ApplyWorkspaceEdit(params)) => {
                        let language_server = language_server!();
                        if language_server.is_initialized() {
                            let offset_encoding = language_server.offset_encoding();
                            let res = self
                                .editor
                                .apply_workspace_edit(offset_encoding, &params.edit);
                            Ok(serde_json::json!(lsp::ApplyWorkspaceEditResponse {
                                applied: res.is_ok(),
                                failure_reason: res.as_ref().err().map(|e| e.kind.to_string()),
                                failed_change: res
                                    .as_ref()
                                    .err()
                                    .map(|e| e.failed_change_idx as u32),
                            }))
                        } else {
                            Err(helix_lsp::jsonrpc::Error {
                                code: helix_lsp::jsonrpc::ErrorCode::InvalidRequest,
                                message: "Server must be initialized to request workspace edits"
                                    .to_string(),
                                data: None,
                            })
                        }
                    }
                    Ok(MethodCall::WorkspaceFolders) => Ok(serde_json::json!(
                        &*language_server!().workspace_folders().await
                    )),
                    Ok(MethodCall::WorkspaceConfiguration(params)) => {
                        let language_server = language_server!();
                        let result: Vec<_> = params
                            .items
                            .iter()
                            .map(|item| {
                                let mut config = match language_server.config() {
                                    Some(c) => c,
                                    None => return serde_json::Value::Null,
                                };
                                if let Some(section) = item.section.as_ref() {
                                    if !section.is_empty() {
                                        for part in section.split('.') {
                                            config = match config.get(part) {
                                                Some(c) => c,
                                                None => return serde_json::Value::Null,
                                            };
                                        }
                                    }
                                }
                                config.clone()
                            })
                            .collect();
                        Ok(serde_json::json!(result))
                    }
                    Ok(MethodCall::RegisterCapability(params)) => {
                        if let Some(client) = self.editor.language_servers.get_by_id(server_id) {
                            for reg in params.registrations {
                                if reg.method == lsp::notification::DidChangeWatchedFiles::METHOD {
                                    if let Some(options) = reg.register_options {
                                        if let Ok(ops) =
                                            serde_json::from_value::<
                                                lsp::DidChangeWatchedFilesRegistrationOptions,
                                            >(options)
                                        {
                                            self.editor
                                                .language_servers
                                                .file_event_handler
                                                .register(
                                                    client.id(),
                                                    std::sync::Arc::downgrade(client),
                                                    reg.id,
                                                    ops,
                                                );
                                        }
                                    }
                                }
                            }
                        }
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::UnregisterCapability(params)) => {
                        for unreg in params.unregisterations {
                            if unreg.method == lsp::notification::DidChangeWatchedFiles::METHOD {
                                self.editor
                                    .language_servers
                                    .file_event_handler
                                    .unregister(server_id, unreg.id);
                            }
                        }
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowDocument(_params)) => {
                        // TODO: implement show document
                        Ok(serde_json::json!(lsp::ShowDocumentResult {
                            success: false
                        }))
                    }
                    Ok(MethodCall::WorkspaceDiagnosticRefresh) => {
                        // TODO: trigger pull diagnostics
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::ShowMessageRequest(params)) => {
                        self.handle_show_message(params.typ, params.message);
                        Ok(serde_json::Value::Null)
                    }
                };

                let language_server = language_server!();
                if let Err(err) = language_server.reply(id.clone(), reply) {
                    log::error!(
                        "Failed to send reply to server '{}' request {id}: {err}",
                        language_server.name()
                    );
                }
            }
            Call::Invalid { id } => log::error!("LSP invalid method call id={id:?}"),
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
