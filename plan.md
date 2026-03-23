# Helide: GUI Frontend for Helix Editor

## Overview

A native GUI frontend for Helix editor, rendering directly to a winit window via wgpu instead of through a terminal emulator. Unlike Neovide (which talks to Neovim over msgpack-RPC), this integrates in-process by replacing the terminal backend.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  helix-core / helix-view / helix-lsp / helix-dap    │  ← unchanged, reused as deps
├─────────────────────────────────────────────────────┤
│  helix-tui  (Buffer, Cell, Terminal<B>, Backend)     │  ← reused, new Backend impl
├─────────────────────────────────────────────────────┤
│  helix-term (Compositor, Component, EditorView, UI)  │  ← reused for components
├─────────────────────────────────────────────────────┤
│  helide (new crate)                                  │
│  ├── GpuBackend (impl Backend)                       │
│  ├── Renderer (wgpu + glyphon)                       │
│  ├── InputMapper (winit KeyEvent → helix Event)      │
│  └── Application (own event loop, owns Terminal<G>)  │
└─────────────────────────────────────────────────────┘
```

## What Makes This Feasible

### Clean Backend trait (`helix-tui/src/backend/mod.rs`)

```rust
pub trait Backend {
    fn claim(&mut self) -> Result<(), io::Error>;
    fn reconfigure(&mut self, config: Config) -> Result<(), io::Error>;
    fn restore(&mut self) -> Result<(), io::Error>;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
        where I: Iterator<Item = (u16, u16, &'a Cell)>;
    fn hide_cursor(&mut self) -> Result<(), io::Error>;
    fn show_cursor(&mut self, kind: CursorKind) -> Result<(), io::Error>;
    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error>;
    fn clear(&mut self) -> Result<(), io::Error>;
    fn size(&self) -> Result<Rect, io::Error>;
    fn flush(&mut self) -> Result<(), io::Error>;
    fn supports_true_color(&self) -> bool;
    fn get_theme_mode(&self) -> Option<helix_view::theme::Mode>;
    fn set_background_color(&mut self, color: Option<Color>) -> io::Result<()>;
}
```

Three implementations already exist: `TerminaBackend`, `CrosstermBackend`, `TestBackend`.

### Clean rendering pipeline

```
Compositor.render(area, surface, ctx)       ← components write to Buffer (Cell grid)
  └→ Terminal.flush()                       ← diffs current vs previous Buffer
      └→ Backend.draw(changed_cells_iter)   ← only changed (x, y, &Cell) tuples
          Backend.flush()                   ← present
```

Components never touch the backend. They write to a `Buffer` (Vec<Cell> indexed by x,y). Each `Cell` has: `symbol: String`, `fg: Color`, `bg: Color`, `underline_color: Color`, `underline_style: UnderlineStyle`, `modifier: Modifier`.

### Helix's own Event type is backend-agnostic (`helix-view/src/input.rs`)

```rust
pub enum Event {
    FocusGained,
    FocusLost,
    Key(KeyEvent),      // code: KeyCode, modifiers: KeyModifiers
    Mouse(MouseEvent),  // kind, column, row, modifiers
    Paste(String),
    Resize(u16, u16),
    IdleTimeout,
}
```

The Compositor and all Components consume this type, not termina events.

## What Makes This Hard

### 1. Application is NOT reusable as-is (Highest Impact)

`helix-term/src/application.rs` is the main coupling point. **You need your own Application struct.** Here's why:

**a) Backend type is hardcoded via `#[cfg]` (line 56-61):**
```rust
#[cfg(all(not(windows), not(feature = "integration")))]
type TerminalBackend = TerminaBackend;
type Terminal = tui::terminal::Terminal<TerminalBackend>;
```

**b) Event stream type is hardcoded (line 64-66):**
```rust
#[cfg(not(windows))]
type TerminalEvent = termina::Event;
```

**c) `handle_terminal_events()` pattern-matches on termina types directly (line 694-760):**
```rust
pub async fn handle_terminal_events(&mut self, event: std::io::Result<TerminalEvent>) {
    let should_redraw = match event.unwrap() {
        termina::Event::WindowResized(..) => { ... }
        termina::Event::Key(termina::event::KeyEvent { kind: Release, .. }) => false,
        termina::Event::Csi(csi::Csi::Mode(csi::Mode::ReportTheme(mode))) => { ... }
        event => self.compositor.handle_event(&event.into(), &mut cx),  // termina→helix conversion
    };
}
```

**d) `event_stream()` creates a termina-specific stream (line 1261-1272):**
```rust
pub fn event_stream(&self) -> impl Stream<Item = std::io::Result<TerminalEvent>> + Unpin {
    let reader = self.terminal.backend().terminal().event_reader();
    termina::EventStream::new(reader, |event| { ... })
}
```

**What you can reuse from Application**: The general structure of `new()` (editor init, theme loading, compositor setup, file opening), `render()` (compositor→terminal→backend), `handle_editor_event()`, `handle_language_server_message()`, `handle_config_events()`, `close()`. Roughly 70% of the code.

**What you must replace**: Backend construction, event stream, `handle_terminal_events()`, the event loop (`event_loop_until_idle`), signal handling (SIGTSTP/SIGCONT are terminal-specific), `run()`.

### 2. Event Loop Mismatch (Highest Complexity)

Helix: `tokio::select!` in an async loop polling multiple streams.
winit: `EventLoop::run()` takes a callback, owns the main thread, never returns.

**Recommended approach: winit on main thread, tokio on a background thread.**

```rust
fn main() {
    let event_loop = EventLoop::new().unwrap();
    let (winit_tx, winit_rx) = tokio::sync::mpsc::unbounded_channel();

    // Spawn tokio runtime on a background thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut app = HelideApp::new(winit_rx, ...).await;
            app.run().await;
        });
    });

    // winit owns the main thread
    event_loop.run(move |event, elwt| {
        // Convert winit events → helix Events, send via channel
        if let Some(helix_event) = convert_event(&event) {
            winit_tx.send(helix_event).ok();
        }
    });
}
```

Then in the helide event loop:
```rust
loop {
    tokio::select! {
        biased;
        Some(event) = winit_rx.recv() => {
            self.handle_event(event).await;
        }
        // ... same job/callback/editor-event arms as helix-term
    }
}
```

But rendering is tricky: the wgpu surface/swapchain belongs to the winit window (main thread). Options:
- **a)** Render on the main thread in the winit callback, read cell grid from shared state
- **b)** Use `wgpu` from the tokio thread (wgpu is thread-safe), only window creation on main thread
- **c)** Send "please redraw" message back to winit thread, which does the GPU work

Option (b) is simplest — create the window and wgpu surface on the main thread, then move the surface to the tokio thread. winit events flow in via channel, render commands flow to GPU from the tokio thread.

### 3. Input Translation (Medium, Tedious)

winit's `KeyEvent` → helix's `KeyEvent`. The mapping is 1:1 for most keys but has quirks:

| winit | helix |
|-------|-------|
| `Key::Named(NamedKey::Backspace)` | `KeyCode::Backspace` |
| `Key::Named(NamedKey::Enter)` | `KeyCode::Enter` |
| `Key::Character(s)` | `KeyCode::Char(s.chars().next())` |
| `Key::Named(NamedKey::F1)` etc | `KeyCode::F(1)` |
| `ModifiersState::SHIFT` | `KeyModifiers::SHIFT` |
| `ModifiersState::CONTROL` | `KeyModifiers::CONTROL` |
| `ModifiersState::ALT` | `KeyModifiers::ALT` |
| `ModifiersState::SUPER` | `KeyModifiers::SUPER` |

Helix has ~30 KeyCode variants + modifiers + mouse events. Maybe 200-300 lines of conversion code. Reference: the existing `From<termina::event::KeyEvent>` impl in `helix-view/src/input.rs:525`.

### 4. Cell-to-GPU Rendering (Medium-High)

`Backend::draw()` receives an iterator of `(u16, u16, &Cell)` — only the changed cells.

GpuBackend strategy:
```rust
struct GpuBackend {
    grid: Vec<Cell>,           // full cell grid, updated incrementally
    width: u16, height: u16,
    renderer: GpuRenderer,     // wgpu + glyphon
    cursor_pos: (u16, u16),
    cursor_kind: CursorKind,
}

impl Backend for GpuBackend {
    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where I: Iterator<Item = (u16, u16, &'a Cell)>
    {
        for (x, y, cell) in content {
            self.grid[(y as usize) * (self.width as usize) + (x as usize)] = cell.clone();
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        self.renderer.render_grid(&self.grid, self.width, self.height,
                                   self.cursor_pos, self.cursor_kind);
        Ok(())
    }

    fn size(&self) -> Result<Rect, io::Error> {
        Ok(Rect::new(0, 0, self.width, self.height))
    }

    fn supports_true_color(&self) -> bool { true }
    // ...
}
```

For the actual GPU rendering, see "Rendering Pipeline" section below.

### 5. Config/Terminal Features (Low)

`Config` passed to `reconfigure()` has:
- `enable_mouse_capture` → always on (window always receives mouse)
- `force_enable_extended_underlines` → always supported
- `kitty_keyboard_protocol` → not applicable (you handle keys natively)

Just ignore these or return Ok.

## Rendering Pipeline: Reusing Alacritty's Approach

### What's reusable from Alacritty

**`crossfont` crate (highly reusable, use directly)**

Alacritty's standalone font rasterization library. Apache-2.0, on crates.io.

```rust
// Core API:
trait Rasterize {
    fn new(device_pixel_ratio: f32) -> Result<Self>;
    fn load_font(desc: &FontDesc, size: Size) -> Result<FontKey>;
    fn metrics(key: FontKey, size: Size) -> Result<Metrics>;
    fn get_glyph(key: GlyphKey) -> Result<RasterizedGlyph>;
}

// Output:
struct RasterizedGlyph {
    character: char,
    width: i32, height: i32,
    top: i32, left: i32,       // baseline offsets
    advance: (i32, i32),       // cursor movement
    buffer: BitmapBuffer,      // raw pixel data (RGB or RGBA)
}
```

Platform backends: Core Text (macOS), FreeType + Fontconfig (Linux), DirectWrite (Windows). Designed for monospace — exactly what we need.

**`alacritty_terminal` — NOT useful.** It's a terminal emulator (PTY, ANSI parsing). Helide doesn't need terminal emulation.

**Alacritty's renderer — NOT directly reusable.** It's OpenGL (not wgpu), lives in the `alacritty` binary crate (not a library), and is coupled to Alacritty's display loop. But the architecture patterns are the reference design.

### Rendering approach: crossfont + wgpu atlas (Alacritty's pattern, modernized)

This follows exactly what Alacritty does, but with wgpu instead of OpenGL:

```
crossfont::Rasterize          wgpu texture atlas         wgpu render pass
  load_font(desc, size)   →   upload RasterizedGlyph   →   for each cell:
  get_glyph(key)              bitmaps to atlas texture       1. draw bg quad (color from Cell.bg)
  metrics(key, size)          track UV coords per glyph      2. draw glyph quad (sample atlas)
                                                              3. draw decorations (underline, etc.)
```

**Why crossfont over glyphon/cosmic-text:**
- crossfont is purpose-built for monospace terminal grids — no layout engine overhead
- Gives raw bitmaps you feed directly into a texture atlas
- Same rasterization quality as Alacritty (proven at scale)
- glyphon/cosmic-text include text shaping and layout engines designed for proportional text — unnecessary complexity for a fixed cell grid

**The atlas renderer you write (~500-800 lines):**

```rust
struct GlyphAtlas {
    texture: wgpu::Texture,
    /// UV coordinates for each cached glyph
    cache: HashMap<GlyphKey, AtlasEntry>,
    /// Current packing position in the atlas
    cursor: (u32, u32),
    row_height: u32,
}

struct AtlasEntry {
    uv: [f32; 4],  // (u0, v0, u1, v1)
    metrics: GlyphMetrics,
}

struct CellRenderer {
    atlas: GlyphAtlas,
    bg_pipeline: wgpu::RenderPipeline,    // solid color quads for backgrounds
    glyph_pipeline: wgpu::RenderPipeline, // textured quads sampling the atlas
    rect_pipeline: wgpu::RenderPipeline,  // underlines, cursor, strikethrough
    cell_size: (f32, f32),
    instance_buffer: wgpu::Buffer,        // per-cell instance data
}
```

**Render loop (2-3 draw calls per frame, same as Alacritty):**

1. **Background pass**: For each cell, emit a colored quad at `(x * cell_w, y * cell_h)`. Use instanced rendering — one draw call for all backgrounds.
2. **Glyph pass**: For each non-space cell, look up glyph in atlas (rasterize + upload on cache miss), emit a textured quad. One draw call with instancing.
3. **Decoration pass**: Underlines, cursor, strikethrough as thin colored rects. One draw call.

**Per-cell instance data (GPU buffer):**
```rust
#[repr(C)]
struct CellInstance {
    pos: [f32; 2],           // pixel position
    bg_color: [f32; 4],      // RGBA
    fg_color: [f32; 4],      // RGBA
    glyph_uv: [f32; 4],      // atlas UV (0,0,0,0 for spaces)
    glyph_offset: [f32; 2],  // bearing offset within cell
    flags: u32,               // bold, italic, underline, cursor, etc.
}
```

**WGSL shaders (~100 lines total):**
- Background vertex/fragment: position quad, output solid color
- Glyph vertex/fragment: position quad with bearing offset, sample atlas texture, tint with fg_color
- Rect vertex/fragment: thin quads for underlines/cursor

### Handling helix Cell → GPU

Map from helix-tui `Cell` fields:

| Cell field | GPU mapping |
|-----------|-------------|
| `symbol: String` | Look up in crossfont → atlas UV |
| `fg: Color` | Convert to RGBA (see color mapping below) |
| `bg: Color` | Convert to RGBA |
| `underline_style` | Set flag bits, draw in decoration pass |
| `underline_color` | Decoration pass color |
| `modifier: Modifier` | See modifier mapping below |

**Color mapping** (`helix_view::graphics::Color` → `[f32; 4]`):
- `Color::Rgb(r, g, b)` → `[r/255, g/255, b/255, 1.0]`
- `Color::Indexed(n)` → lookup in 256-color ANSI palette table
- Named colors (`Red`, `Blue`, etc.) → ANSI color palette
- `Color::Reset` → use theme default fg/bg

**Modifier mapping:**
- `BOLD` → load bold font variant from crossfont (`Style::Bold`)
- `ITALIC` → load italic variant (`Style::Italic`)
- `BOLD | ITALIC` → bold-italic variant
- `DIM` → multiply fg alpha by 0.5
- `REVERSED` → swap fg/bg colors before uploading
- `HIDDEN` → set fg = bg
- `CROSSED_OUT` → set strikethrough flag, draw in decoration pass

## Key Files Reference

| File | What to reuse | What to replace |
|------|--------------|-----------------|
| `helix-tui/src/backend/mod.rs` | `Backend` trait | — |
| `helix-tui/src/terminal.rs` | `Terminal<B>`, double-buffering, diff | — |
| `helix-tui/src/buffer.rs` | `Buffer`, `Cell` | — |
| `helix-term/src/compositor.rs` | `Compositor`, `Component`, `Context`, `Event` | — |
| `helix-term/src/ui/*` | All UI components (EditorView, Picker, Prompt, etc.) | — |
| `helix-term/src/application.rs` | ~70% (init, render, editor events, LSP, close) | Event loop, event handling, backend init |
| `helix-term/src/main.rs` | Args, config loading | Entry point (winit event loop) |
| `helix-view/src/input.rs` | `Event`, `KeyEvent`, `MouseEvent` types | — |
| `helix-view/src/keyboard.rs` | `KeyCode`, `KeyModifiers` | — |

## Implementation Phases

### Phase 0: Crate Skeleton
- Create `helide` crate with dependencies on helix-tui, helix-term, helix-view, helix-core, helix-lsp
- Verify it compiles and can access `Backend`, `Terminal<B>`, `Compositor`, `Editor`

### Phase 1: Stub GpuBackend + Static Render
- Implement `Backend` trait with a winit window + wgpu surface
- `draw()` updates an internal cell grid, `flush()` renders the full grid
- Use crossfont for glyph rasterization → build wgpu texture atlas
- Write WGSL shaders for background quads + textured glyph quads
- Test with a manually constructed `Buffer` — no editor logic yet, just prove cells render

### Phase 2: Wire Up the Editor (Core Milestone)
- Write a helide `Application` struct that:
  - Reuses `Application::new()`-style init (Editor, Compositor, theme, file opening)
  - Uses `Terminal<GpuBackend>` instead of `Terminal<TerminaBackend>`
  - Has its own `render()` that calls `compositor.render() → terminal.draw() → backend.flush()`
- At this point: editor state renders to the window, but no input yet

### Phase 3: Event Loop Integration
- Set up winit on main thread, tokio runtime on background thread
- Channel bridge: winit events → tokio select loop
- Integrate with existing helix async machinery (jobs, LSP, editor events)
- Handle window resize → `terminal.resize()` + `compositor.resize()`
- Handle window close → `editor.close()`
- Handle focus gain/loss → `Event::FocusGained/Lost`

### Phase 4: Input Mapping
- `winit::event::KeyEvent` → `helix_view::input::KeyEvent` conversion
- `winit::event::MouseButton/cursor` → `helix_view::input::MouseEvent` conversion
- Handle IME/compose input (winit `Ime` events → `Event::Paste` or character events)
- Filter key release events (helix only cares about presses)

### Phase 5: Full Styling
- Map all `Color` variants to RGB for GPU: named colors need a palette lookup
- `Modifier::BOLD` → use bold font variant or synthetic bold
- `Modifier::ITALIC` → italic font variant
- `Modifier::DIM` → reduce alpha
- `Modifier::REVERSED` → swap fg/bg
- `UnderlineStyle::{Line, Curl, Dotted, Dashed, Double}` → draw decorations under glyphs
- `Modifier::CROSSED_OUT` → strikethrough line

### Phase 6: Polish
- Smooth scrolling (interpolate scroll position between frames)
- Cursor animation (fade, position interpolation)
- Configurable fonts (family, size, line height)
- DPI/scale factor handling (winit provides this)
- Ligature support (needs harfbuzz or cosmic-text shaping)
- System clipboard integration (already works via helix-view, but might need arboard)
- OS dark/light mode → `get_theme_mode()` via winit's theme detection

## Dependency Choices

| Need | Recommendation | Why |
|------|---------------|-----|
| Windowing | winit | Standard, cross-platform |
| GPU | wgpu | Vulkan/Metal/DX12 abstraction, Rust-native |
| Font rasterization | crossfont (from Alacritty) | Monospace-focused, cross-platform (CoreText/FreeType/DirectWrite), proven |
| Glyph atlas + rendering | Custom (~500-800 LOC) | Follows Alacritty's 2-pass pattern, adapted for wgpu |

**Alternatives considered:**
- **glyphon** — wgpu-native text renderer, but includes layout engine overhead unnecessary for fixed grid
- **cosmic-text** — full text shaping, good if you want ligatures/complex scripts later, but heavier
- **font-kit** — font loading only (no rasterization), less battle-tested than crossfont for terminals

## Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| Event loop integration | High | Prototype the winit↔tokio bridge first, before writing any rendering code |
| helix-term internal API changes | Medium | Pin to a specific helix commit; the crate isn't published, so API isn't stable |
| IME / complex input | Medium | Start with ASCII, add IME later |
| Text rendering performance | Low | crossfont + wgpu atlas is Alacritty's proven approach; 2 draw calls/frame scales to any screen size |
| Multi-width characters (CJK) | Medium | Cell grid already handles this (wide chars span 2 cells); rendering needs to match |
