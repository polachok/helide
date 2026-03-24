# Helide: GUI Frontend for Helix Editor

## Overview

A native GUI frontend for Helix editor, rendering directly to a winit window via wgpu instead of through a terminal emulator. Unlike Neovide (which talks to Neovim over msgpack-RPC), this integrates in-process by replacing the terminal backend.

## Current Status

Fully functional GPU-rendered Helix editor with native macOS integration:

- **GPU-accelerated rendering** via wgpu + crossfont glyph atlas (3-pass instanced)
- **Full Helix editor** with compositor, keymaps, syntax highlighting, themes
- **Keyboard and mouse input** mapped from winit to helix events
- **LSP integration** — diagnostics, progress, spinners, language server messages
- **Native macOS app** — menu bar, transparent titlebar, Open With, Open Recent, drag-and-drop
- **macOS .app bundle + DMG packaging** with bundled helix runtime

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  helix-core / helix-view / helix-lsp / helix-dap    │  ← git submodule, pinned rev
├─────────────────────────────────────────────────────┤
│  helix-tui  (Buffer, Cell, Terminal<B>, Backend)     │  ← reused, GpuBackend impl
├─────────────────────────────────────────────────────┤
│  helix-term (Compositor, Component, EditorView, UI)  │  ← reused for all UI
├─────────────────────────────────────────────────────┤
│  helide (binary crate)                               │
│  ├── backend.rs        — GpuBackend (impl Backend)   │
│  ├── renderer.rs       — wgpu + crossfont + atlas    │
│  ├── input.rs          — winit → helix events        │
│  ├── app.rs            — HelideApp + LSP handling     │
│  ├── config.rs         — font config (~/.config/helide) │
│  ├── main.rs           — winit event loop, wgpu init  │
│  ├── platform/macos.rs — native menus, open-with, etc │
│  └── shaders/          — bg.wgsl, glyph.wgsl         │
└─────────────────────────────────────────────────────┘
```

### Key Design Decisions

**Event loop**: winit owns the main thread via `run_app()`. Tokio runtime entered on main thread so `tokio::spawn` works. Periodic 100ms redraw timer via `EventLoopProxy` for LSP updates.

**LSP handling**: Async via `tokio::select!` with 5ms timeout. Polls LSP incoming, job callbacks, save queue, and status messages. Full `handle_language_server_message` ported from helix-term (diagnostics, progress, workspace edits, config, capabilities).

**Handlers**: Constructed manually with dummy tokio channels for LSP features and real `word_index::Handler`. Basic editing + LSP diagnostics work; completion/signature help handlers not spawned.

**Rendering**: 3-pass instanced rendering per frame:
1. Background quads (one draw call for all cells)
2. Glyph quads sampling texture atlas (one draw call, gamma-corrected alpha)
3. Decoration quads — underlines, strikethrough, cursor (one draw call)

**Colors**: Non-sRGB surface format, colors passed as sRGB. Theme default fg/bg extracted from `ui.background` and `ui.text` scopes. Auto-updates on theme change.

**Font**: crossfont with regular/bold/italic/bold-italic variants. Nearest-neighbor atlas sampling. Font size scaled by DPI. Glyph alpha gamma correction (pow 1.4) to match terminal emulator weight. RGB sub-pixel AA averaged to grayscale. Cell width ceiled for proper centering.

**macOS integration**: objc2 0.6 for native APIs. Delegate subclassing for `application:openFiles:`. `NSDocumentController` for Open Recent. Transparent titlebar with auto light/dark appearance based on theme luminance.

## Source Files

| File | Purpose |
|------|---------|
| `src/main.rs` | winit event loop, wgpu init, runtime discovery, CLI args, macOS setup |
| `src/app.rs` | HelideApp: Editor + Compositor + Terminal init, event/render loop, LSP handler |
| `src/renderer.rs` | wgpu pipelines, glyph atlas, cell-to-GPU rendering, color mapping, decorations |
| `src/backend.rs` | GpuBackend implementing helix-tui Backend trait |
| `src/input.rs` | winit → helix event conversion (keys, mouse, scroll accumulator) |
| `src/config.rs` | Font config from ~/.config/helide/config.toml |
| `src/platform/macos.rs` | Native menu bar, NSOpenPanel, Open Recent, file open handler, appearance |
| `src/shaders/bg.wgsl` | Background quad vertex/fragment shader |
| `src/shaders/glyph.wgsl` | Glyph quad shader with gamma-corrected alpha |
| `extra/osx/` | macOS .app template (Info.plist, icon) |
| `macos-builder/run` | Build script: compiles, fetches/builds grammars, creates .app + .dmg |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| helix-* | git rev 3d68e0a | Editor core, TUI, view, LSP, etc. (submodule) |
| winit | 0.30 | Window creation, event loop |
| wgpu | 24 | GPU rendering (Metal/Vulkan/DX12) |
| crossfont | 0.9 | Font rasterization (CoreText/FreeType/DirectWrite) |
| tokio | 1 | Async runtime for helix machinery |
| objc2 | 0.6 | macOS native APIs (menus, file handling) |
| arboard | 3 | System clipboard |
| bytemuck | 1 | GPU buffer casting |
| content_inspector | 0.2 | Text file detection for drag-and-drop |

## What Works

### Editor
- Normal mode, insert mode, command mode
- All helix keybindings and commands
- Syntax highlighting (tree-sitter via helix runtime)
- Theme support with live switching (`:theme`)
- File picker, fuzzy finder, command palette
- Mouse clicks, scroll (accumulator-gated), selection
- Window resize with cell grid recalculation
- Multiple buffers and splits
- Line numbers, status line, mode indicator
- Bold, italic, dim, reversed, hidden text modifiers
- Underline styles: line, double, curl, dotted, dashed
- Strikethrough, cursor rendering (block, bar, underline)

### LSP
- Diagnostics (publishDiagnostics)
- Progress messages and spinner animation
- Language server lifecycle (init, exit, capabilities)
- Workspace edits, configuration, file watchers
- Document save with file size display

### macOS Native
- Menu bar: Helide, File, Edit, Window, Help
- File > New, Open (NSOpenPanel), Open Recent, Open Directory, Save, Close
- Edit > Undo, Redo, Paste (via arboard)
- Help > Helix Tutor
- Window > Minimize, Zoom, Full Screen
- Transparent titlebar matching editor background
- Auto light/dark titlebar based on theme luminance
- "Open With" from Finder (application:openFiles: delegate)
- Drag-and-drop files onto window and dock icon
- Close button hides window (macOS behavior), dock icon reopens
- Graceful shutdown: flush writes, close LSP servers

### Packaging
- macOS .app bundle with bundled helix runtime (themes, queries, grammars, tutor)
- DMG creation (hdiutil or create-dmg)
- App icon
- `INSTALL=true` option for /Applications + LaunchServices registration
- helix as git submodule with grammar fetch/build

## Config

`~/.config/helide/config.toml`:
```toml
[font]
family = "JetBrainsMono Nerd Font Mono"
size = 14.0
```

## Building

```sh
# Development
cargo run -- file.rs

# Release + install
INSTALL=true ./macos-builder/run

# Release + DMG
GENERATE_DMG=true ./macos-builder/run
```

## Terminal Integration

### Overview

Embedded terminal emulator pane below the editor, using `alacritty_terminal` for VT parsing/PTY management and the existing wgpu renderer for display. The split is at the GUI level (not Helix's view tree).

### Architecture

```
┌─────────────────────────────────┐
│                                 │
│         Editor (wgpu)           │
│     (existing HelideApp)        │
│                                 │
├─────────────── ─ ─ ─ ──────────┤  ← draggable divider
│                                 │
│      Terminal (wgpu)            │
│   (alacritty_terminal grid)     │
│                                 │
└─────────────────────────────────┘
```

- Top-level `Layout` struct owns split ratio, computes pixel rects for editor/divider/terminal regions
- Terminal hidden by default, toggled via keybind (e.g. Ctrl+`)
- When hidden, editor gets full window
- Start with single terminal session, designed so tabs can be added later

### Rendering

Each region renders to its own offscreen texture, then a composite pass blits both to the swapchain:

- Editor and terminal each have independent instance buffers (background, glyph, decoration)
- Each region tracks a dirty flag — only rebuild instances when content changed
- Composite pass: trivial shader sampling two textures
- Shared glyph atlas — terminal uses same font, same atlas lookups
- Enables future: per-region effects (dim unfocused pane, vibrancy), different frame rates, resize animations, tabs as additional textures

### Terminal Component (TerminalPane)

```
TerminalPane
├── alacritty_terminal::Term<EventListener>  — terminal state machine + grid
├── alacritty_terminal::tty::Pty             — PTY handle (read/write)
├── focused: bool
├── scrollback_offset: usize
└── size: (cols, rows)
```

**Lifecycle:**
1. First toggle-open: spawn PTY with `$SHELL`, create `Term` with grid dimensions from pane size + cell metrics
2. PTY output read on background tokio task, fed into `Term` for parsing
3. Each frame: read `Term` grid cells → convert to renderer instances → draw with same 3-pass pipeline
4. On resize (window or divider drag): notify `Term` of new size, send `SIGWINCH` to PTY
5. On toggle-hide: PTY keeps running, just stop rendering
6. On app exit: kill PTY child process

### Input Routing

Processing order:
1. **Global keybinds** — toggle terminal (intercepted before any component sees it)
2. **Divider drag** — mouse down/move/up on divider region
3. **Route to focused component** — editor or terminal

Focus model:
- Click in editor region → editor focused
- Click in terminal region → terminal focused
- Toggle keybind switches focus along with visibility

When terminal focused: keyboard encoded as escape sequences via alacritty_terminal, written to PTY. Mouse events forwarded if terminal app requests mouse reporting.

### New Files

```
src/
├── layout.rs          — Layout: split ratio, rects, divider drag state, toggle
├── terminal/
│   ├── mod.rs         — TerminalPane: PTY lifecycle, grid reading, dirty tracking
│   ├── input.rs       — key/mouse → terminal escape sequence encoding
│   └── cells.rs       — alacritty_terminal Cell → renderer instance conversion
├── compositor.rs      — composites region textures to swapchain
├── shaders/
│   └── composite.wgsl — samples two textures, outputs to screen
```

### Modified Files

- `renderer.rs` — render to offscreen texture instead of swapchain; accept region rect parameter
- `app.rs` — owns Layout + TerminalPane, routes input, triggers per-region renders
- `input.rs` — add global keybind interception before routing
- `main.rs` — minor: pass through terminal config
- `config.rs` — add terminal toggle keybind config

### Dependencies

- `alacritty_terminal` — VT parsing, PTY management, terminal grid state

## Known Limitations / TODO

### High Priority
- **Completion/signature help** — LSP handlers use dummy channels. Need to spawn actual handlers (requires making helix-term `handlers` module public or reimplementing)
- **IME input** — winit `Ime` events not handled, breaks CJK/compose input

### Medium Priority
- **Wide characters (CJK)** — cell grid handles 2-cell-wide chars but renderer doesn't skip the second cell
- **Cursor blinking** — no animation, cursor is always solid
- **Font smoothing** — gamma correction in shader approximates correct weight; proper fix needs per-CGContext control (like ghostty)

### Nice to Have
- Smooth scrolling (needs separate viewport texture, not just cell offset)
- Cursor movement animation (Neovide-style)
- Ligature support (needs text shaping via harfbuzz or cosmic-text)
- GPU-accelerated curly underlines (currently approximated as thick line)
- Window transparency / vibrancy
- Multi-window support
- Configurable gamma correction for font weight
