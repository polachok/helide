/// GUI-level vertical split: editor (top) + terminal (bottom).
/// Manages split ratio, pixel rects, divider drag, and toggle state.

pub struct Layout {
    /// Terminal pane visible?
    pub terminal_visible: bool,
    /// Fraction of window height for editor (0.0–1.0). Terminal gets the rest.
    pub split_ratio: f32,
    /// Drag state: if Some, the y-coordinate where drag started.
    drag_start_y: Option<f32>,
    /// Ratio when drag began (to compute delta).
    drag_start_ratio: f32,
    /// Total window size in pixels.
    window_width: u32,
    window_height: u32,
}

/// Pixel regions computed from the layout.
pub struct Regions {
    /// Editor region (x, y, w, h) in pixels.
    pub editor: (u32, u32, u32, u32),
    /// Divider region (x, y, w, h) in pixels. None if terminal hidden.
    pub divider: Option<(u32, u32, u32, u32)>,
    /// Terminal region (x, y, w, h) in pixels. None if terminal hidden.
    pub terminal: Option<(u32, u32, u32, u32)>,
}

const DIVIDER_HEIGHT: u32 = 2;
const MIN_RATIO: f32 = 0.15;
const MAX_RATIO: f32 = 0.85;
const DEFAULT_RATIO: f32 = 0.7;

impl Layout {
    pub fn new(window_width: u32, window_height: u32) -> Self {
        Self {
            terminal_visible: false,
            split_ratio: DEFAULT_RATIO,
            drag_start_y: None,
            drag_start_ratio: DEFAULT_RATIO,
            window_width,
            window_height,
        }
    }

    /// Recompute after window resize.
    pub fn set_window_size(&mut self, width: u32, height: u32) {
        self.window_width = width;
        self.window_height = height;
    }

    /// Toggle terminal visibility. Returns true if now visible.
    pub fn toggle_terminal(&mut self) -> bool {
        self.terminal_visible = !self.terminal_visible;
        self.terminal_visible
    }

    /// Compute pixel regions for editor, divider, and terminal.
    pub fn regions(&self) -> Regions {
        let w = self.window_width;
        let h = self.window_height;

        if !self.terminal_visible {
            return Regions {
                editor: (0, 0, w, h),
                divider: None,
                terminal: None,
            };
        }

        let usable = h.saturating_sub(DIVIDER_HEIGHT);
        let editor_h = (usable as f32 * self.split_ratio) as u32;
        let terminal_h = usable - editor_h;

        Regions {
            editor: (0, 0, w, editor_h),
            divider: Some((0, editor_h, w, DIVIDER_HEIGHT)),
            terminal: Some((0, editor_h + DIVIDER_HEIGHT, w, terminal_h)),
        }
    }

    // --- Divider drag ---

    /// Returns true if (x, y) is within the divider hitbox (wider than visual).
    pub fn hit_test_divider(&self, _x: f32, y: f32) -> bool {
        if !self.terminal_visible {
            return false;
        }
        let regions = self.regions();
        if let Some((_, dy, _, dh)) = regions.divider {
            let hitbox = 6.0; // pixels of grab tolerance
            let center = dy as f32 + dh as f32 / 2.0;
            (y - center).abs() <= hitbox
        } else {
            false
        }
    }

    /// Begin a divider drag.
    pub fn drag_start(&mut self, y: f32) {
        self.drag_start_y = Some(y);
        self.drag_start_ratio = self.split_ratio;
    }

    /// Update split ratio during drag. Returns true if ratio changed.
    pub fn drag_update(&mut self, y: f32) -> bool {
        let Some(start_y) = self.drag_start_y else {
            return false;
        };
        let usable = (self.window_height - DIVIDER_HEIGHT) as f32;
        if usable <= 0.0 {
            return false;
        }
        let delta = (y - start_y) / usable;
        let new_ratio = (self.drag_start_ratio + delta).clamp(MIN_RATIO, MAX_RATIO);
        if (new_ratio - self.split_ratio).abs() < 0.001 {
            return false;
        }
        self.split_ratio = new_ratio;
        true
    }

    /// End the drag.
    pub fn drag_end(&mut self) {
        self.drag_start_y = None;
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag_start_y.is_some()
    }

    /// Determine which region a point falls in.
    pub fn region_at(&self, _x: f32, y: f32) -> RegionKind {
        if !self.terminal_visible {
            return RegionKind::Editor;
        }
        let regions = self.regions();
        if let Some((_, ty, _, _)) = regions.terminal {
            if y >= ty as f32 {
                return RegionKind::Terminal;
            }
        }
        if let Some((_, dy, _, dh)) = regions.divider {
            if y >= dy as f32 && y < (dy + dh) as f32 {
                return RegionKind::Divider;
            }
        }
        RegionKind::Editor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Editor,
    Divider,
    Terminal,
}
