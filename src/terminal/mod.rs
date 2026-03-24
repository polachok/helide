pub mod cells;
pub mod input;

use std::borrow::Cow;
use std::sync::Arc;

use alacritty_terminal::event::{Event as AlacTermEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, Msg as PtyMsg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty;
use tokio::sync::mpsc;

use crate::renderer::Renderer;

#[derive(Clone)]
pub struct JsonEventProxy {
    sender: mpsc::UnboundedSender<AlacTermEvent>,
    winit_proxy: winit::event_loop::EventLoopProxy<crate::UserEvent>,
}

impl EventListener for JsonEventProxy {
    fn send_event(&self, event: AlacTermEvent) {
        let is_wakeup = matches!(event, AlacTermEvent::Wakeup);
        let _ = self.sender.send(event);
        // Wake up the winit event loop so it redraws immediately
        if is_wakeup {
            let _ = self.winit_proxy.send_event(crate::UserEvent::Redraw);
        }
    }
}

/// A simple struct implementing `Dimensions` for creating/resizing a `Term`.
struct TermDims {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TerminalPane {
    pub term: Arc<FairMutex<Term<JsonEventProxy>>>,
    notifier: Notifier,
    event_rx: mpsc::UnboundedReceiver<AlacTermEvent>,
    region_texture: wgpu::Texture,
    region_view: wgpu::TextureView,
    region_width: u32,
    region_height: u32,
    pub focused: bool,
    pub dirty: bool,
    pub exited: bool,
    cols: u16,
    rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl TerminalPane {
    pub fn new(
        renderer: &Renderer,
        width: u32,
        height: u32,
        winit_proxy: winit::event_loop::EventLoopProxy<crate::UserEvent>,
    ) -> anyhow::Result<Self> {
        let cell_width = renderer.cell_width;
        let cell_height = renderer.cell_height;

        let cols = (width as f32 / cell_width).max(2.0) as u16;
        let rows = (height as f32 / cell_height).max(1.0) as u16;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_proxy = JsonEventProxy { sender: event_tx, winit_proxy };

        let config = TermConfig::default();
        let dims = TermDims {
            cols: cols as usize,
            rows: rows as usize,
        };

        let term = Term::new(config, &dims, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: cell_width as u16,
            cell_height: cell_height as u16,
        };

        let pty = tty::new(&tty::Options::default(), window_size, 0)?;

        let pty_event_loop =
            PtyEventLoop::new(term.clone(), event_proxy, pty, false, false)?;

        let notifier = Notifier(pty_event_loop.channel());
        let _pty_thread = pty_event_loop.spawn();

        let (region_texture, region_view) =
            renderer.create_region_texture(width, height);

        Ok(TerminalPane {
            term,
            notifier,
            event_rx,
            region_texture,
            region_view,
            region_width: width,
            region_height: height,
            focused: false,
            dirty: true,
            exited: false,
            cols,
            rows,
            cell_width,
            cell_height,
        })
    }

    /// Drain pending events from the PTY event loop.
    pub fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AlacTermEvent::Wakeup => {
                    self.dirty = true;
                }
                AlacTermEvent::Exit | AlacTermEvent::ChildExit(_) => {
                    self.exited = true;
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    /// Resize the terminal pane.
    pub fn resize(&mut self, renderer: &Renderer, width: u32, height: u32) {
        let new_cols = (width as f32 / self.cell_width).max(2.0) as u16;
        let new_rows = (height as f32 / self.cell_height).max(1.0) as u16;

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            let dims = TermDims {
                cols: new_cols as usize,
                rows: new_rows as usize,
            };
            self.term.lock().resize(dims);

            let window_size = WindowSize {
                num_lines: new_rows,
                num_cols: new_cols,
                cell_width: self.cell_width as u16,
                cell_height: self.cell_height as u16,
            };
            self.notify_pty_resize(window_size);
        }

        if width != self.region_width || height != self.region_height {
            let (region_texture, region_view) =
                renderer.create_region_texture(width, height);
            self.region_texture = region_texture;
            self.region_view = region_view;
            self.region_width = width;
            self.region_height = height;
        }

        self.dirty = true;
    }

    /// Write bytes to the PTY.
    pub fn write_to_pty(&self, bytes: &[u8]) {
        self.notifier.notify(Cow::Owned(bytes.to_vec()));
    }

    /// Send a resize message to the PTY.
    fn notify_pty_resize(&self, window_size: WindowSize) {
        let _ = self.notifier.0.send(PtyMsg::Resize(window_size));
    }

    /// Get the offscreen texture view for compositing.
    pub fn region_view(&self) -> &wgpu::TextureView {
        &self.region_view
    }

    /// Get the region dimensions.
    pub fn region_size(&self) -> (u32, u32) {
        (self.region_width, self.region_height)
    }
}
