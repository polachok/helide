use std::io;

use helix_tui::backend::Backend;
use helix_tui::buffer::Cell;
use helix_view::graphics::{CursorKind, Rect};
use helix_view::theme::Color;

use crate::renderer::Renderer;

pub struct GpuBackend {
    renderer: Renderer,
    grid: Vec<Cell>,
    cols: u16,
    rows: u16,
    cursor_pos: Option<(u16, u16)>,
    cursor_kind: CursorKind,
    // Offscreen texture for editor region
    region_texture: wgpu::Texture,
    region_view: wgpu::TextureView,
    region_width: u32,
    region_height: u32,
    dirty: bool,
}

impl GpuBackend {
    pub fn new(renderer: Renderer) -> Self {
        let cols = (renderer.config.width as f32 / renderer.cell_width) as u16;
        let usable_height = renderer.config.height as f32 - renderer.padding_top;
        let rows = (usable_height / renderer.cell_height) as u16;
        let grid = vec![Cell::default(); (cols as usize) * (rows as usize)];

        let width = renderer.config.width;
        let height = renderer.config.height;
        let (region_texture, region_view) = renderer.create_region_texture(width, height);

        GpuBackend {
            renderer,
            grid,
            cols,
            rows,
            cursor_pos: None,
            cursor_kind: CursorKind::Hidden,
            region_texture,
            region_view,
            region_width: width,
            region_height: height,
            dirty: true,
        }
    }

    pub fn cell_width(&self) -> f32 {
        self.renderer.cell_width
    }

    pub fn cell_height(&self) -> f32 {
        self.renderer.cell_height
    }

    pub fn set_default_colors(&mut self, fg: [f32; 4], bg: [f32; 4]) {
        self.renderer.default_fg = fg;
        self.renderer.default_bg = bg;
    }

    /// Resize the editor region (texture + grid). Does NOT resize the swapchain surface.
    /// Call `handle_window_resize()` for actual window resize.
    pub fn handle_resize(&mut self, width: u32, height: u32) {
        let new_cols = (width as f32 / self.renderer.cell_width) as u16;
        let usable_height = height as f32 - self.renderer.padding_top;
        let new_rows = (usable_height / self.renderer.cell_height) as u16;
        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;
            self.grid = vec![Cell::default(); (new_cols as usize) * (new_rows as usize)];
        }
        if width != self.region_width || height != self.region_height {
            let (region_texture, region_view) =
                self.renderer.create_region_texture(width, height);
            self.region_texture = region_texture;
            self.region_view = region_view;
            self.region_width = width;
            self.region_height = height;
        }
    }

    /// Resize the swapchain surface to the full window size.
    /// Call this only on actual window resize, not on region resize.
    pub fn handle_window_resize(&mut self, width: u32, height: u32) {
        self.renderer.resize(width, height);
    }

    pub fn region_view(&self) -> &wgpu::TextureView {
        &self.region_view
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        &mut self.renderer
    }
}

impl Backend for GpuBackend {
    fn claim(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn reconfigure(&mut self, _config: helix_tui::terminal::Config) -> Result<(), io::Error> {
        Ok(())
    }

    fn restore(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            let idx = (y as usize) * (self.cols as usize) + (x as usize);
            if idx < self.grid.len() {
                self.grid[idx] = cell.clone();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), io::Error> {
        self.cursor_kind = CursorKind::Hidden;
        Ok(())
    }

    fn show_cursor(&mut self, kind: CursorKind) -> Result<(), io::Error> {
        self.cursor_kind = kind;
        Ok(())
    }

    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error> {
        self.cursor_pos = Some((x, y));
        Ok(())
    }

    fn clear(&mut self) -> Result<(), io::Error> {
        for cell in &mut self.grid {
            cell.reset();
        }
        Ok(())
    }

    fn size(&self) -> Result<Rect, io::Error> {
        Ok(Rect::new(0, 0, self.cols, self.rows))
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        self.renderer.render_grid_to_texture(
            &self.grid,
            self.cols,
            self.rows,
            self.cursor_pos,
            self.cursor_kind,
            &self.region_view,
            self.region_width,
            self.region_height,
        );
        self.dirty = false;
        Ok(())
    }

    fn supports_true_color(&self) -> bool {
        true
    }

    fn get_theme_mode(&self) -> Option<helix_view::theme::Mode> {
        None
    }

    fn set_background_color(&mut self, _color: Option<Color>) -> io::Result<()> {
        Ok(())
    }
}
