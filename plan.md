# Helide: GUI Frontend for Helix Editor

## Overview

A native GUI frontend for Helix editor, rendering directly to a winit window via wgpu instead of through a terminal emulator. Unlike Neovide (which talks to Neovim over msgpack-RPC), this integrates in-process by replacing the terminal backend.

## Current Status

All core phases are implemented and working:

- **GPU-accelerated rendering** via wgpu + crossfont glyph atlas
- **Full Helix editor** with compositor, keymaps, syntax highlighting, themes
- **Keyboard and mouse input** mapped from winit to helix events
- **Decoration rendering** (underlines, strikethrough, cursor)
- **File opening** from CLI args
- **Runtime auto-discovery** (dev checkout, ~/.config/helix, next to hx binary)
- **Clean shutdown** without macOS crash dialog

## Architecture (Implemented)

```
┌─────────────────────────────────────────────────────┐
│  helix-core / helix-view / helix-lsp / helix-dap    │  ← git dep, pinned rev
├─────────────────────────────────────────────────────┤
│  helix-tui  (Buffer, Cell, Terminal<B>, Backend)     │  ← reused, GpuBackend impl
├─────────────────────────────────────────────────────┤
│  helix-term (Compositor, Component, EditorView, UI)  │  ← reused for all UI
├─────────────────────────────────────────────────────┤
│  helide (binary crate)                               │
│  ├── backend.rs   — GpuBackend (impl Backend)        │
│  ├── renderer.rs  — wgpu + crossfont + glyph atlas   │
│  ├── input.rs     — winit KeyEvent → helix Event     │
│  ├── app.rs       — HelideApp (Editor+Compositor)    │
│  └── main.rs      — winit event loop, wgpu init      │
│  └── shaders/     — bg.wgsl, glyph.wgsl              │
└─────────────────────────────────────────────────────┘
```

### Key Design Decisions

**Event loop**: winit owns the main thread via `run_app()`. Tokio runtime is entered on the main thread (`runtime.enter()`) so `tokio::spawn` works for helix async handlers. No separate thread needed — all rendering and input handling is synchronous in winit callbacks.

**Handlers**: helix-term's `handlers` module is private. We construct `Handlers` manually with dummy tokio channels for LSP features (completions, signature help, etc.) and a real `word_index::Handler`. Basic editing works fully; LSP features require wiring up the actual handler tasks.

**Rendering**: 3-pass instanced rendering per frame:
1. Background quads (one draw call for all cells)
2. Glyph quads sampling texture atlas (one draw call)
3. Decoration quads — underlines, strikethrough, cursor (one draw call)

**Colors**: Non-sRGB surface format to avoid double gamma. Theme default fg/bg extracted from `ui.background` and `ui.text` scopes. Full ANSI 256-color palette + RGB support.

**Font**: crossfont with regular/bold/italic/bold-italic variants. Nearest-neighbor atlas sampling for crisp glyphs. Font size scaled by window DPI factor.

## Source Files

| File | Lines | Purpose |
|------|-------|---------|
| `src/main.rs` | ~230 | winit event loop, wgpu init, runtime discovery, CLI args |
| `src/app.rs` | ~230 | HelideApp: Editor + Compositor + Terminal init, event handling, render loop |
| `src/renderer.rs` | ~760 | wgpu pipelines, glyph atlas, cell-to-GPU rendering, color mapping |
| `src/backend.rs` | ~120 | GpuBackend implementing helix-tui Backend trait |
| `src/input.rs` | ~170 | winit → helix event conversion (keys, mouse, scroll) |
| `src/shaders/bg.wgsl` | ~40 | Background quad vertex/fragment shader |
| `src/shaders/glyph.wgsl` | ~50 | Glyph quad vertex/fragment shader (atlas sampling) |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| helix-* | git rev 3d68e0a | Editor core, TUI, view, LSP, etc. |
| winit | 0.30 | Window creation, event loop |
| wgpu | 24 | GPU rendering (Metal/Vulkan/DX12) |
| crossfont | 0.9 | Font rasterization (CoreText/FreeType/DirectWrite) |
| tokio | 1 | Async runtime for helix machinery |
| bytemuck | 1 | GPU buffer casting |
| pollster | 0.4 | Block on async (wgpu init) |
| dirs | 6 | Platform config directory |
| which | 7 | Find hx binary for runtime discovery |

## What Works

- Normal mode, insert mode, command mode
- All helix keybindings and commands
- Syntax highlighting (tree-sitter via helix runtime)
- Theme support (loads user's configured theme)
- File picker, fuzzy finder, command palette
- Mouse clicks, scroll, selection
- Window resize with cell grid recalculation
- Multiple buffers and splits
- Line numbers, status line, mode indicator
- Bold, italic, dim, reversed, hidden text modifiers
- Underline styles: line, double, curl, dotted, dashed
- Strikethrough
- Cursor rendering (block, bar, underline)

## Known Limitations / TODO

### High Priority
- **LSP not wired up** — Handlers use dummy channels. Need to spawn actual CompletionHandler, SignatureHelpHandler, AutoSaveHandler, etc. from helix-term (requires making `handlers` module public or reimplementing)
- **No async event polling loop** — editor events (LSP responses, jobs) only polled on RedrawRequested, not continuously. Need a timer or EventLoopProxy-based wakeup
- **IME input not handled** — winit `Ime` events are ignored, breaks CJK/compose input

### Medium Priority
- **No clipboard integration** — helix-view has clipboard support but may need platform adaptation
- **Wide characters (CJK)** — cell grid handles 2-cell-wide chars but renderer doesn't skip the second cell
- **Cursor blinking** — no animation, cursor is always solid
- **OS dark/light mode** — `get_theme_mode()` returns None, could use winit theme detection
- **Font configuration** — hardcoded to "monospace" at 16pt, should read from config

### Nice to Have
- Smooth scrolling (interpolate between frames)
- Cursor movement animation (Neovide-style)
- Ligature support (needs text shaping via harfbuzz or cosmic-text)
- GPU-accelerated curly underlines (currently approximated as thick line)
- Window transparency
- Multi-window support
- Custom title bar

## Completed Implementation Phases

### Phase 0: Crate Skeleton [done]
Created helide crate with git dependencies on helix crates at pinned commit. Verified Backend trait is implementable from outside.

### Phase 1: GpuBackend + Static Render [done]
Implemented Backend trait with winit + wgpu. Built crossfont glyph atlas with texture upload. Wrote WGSL shaders for instanced background and glyph quads. Proved rendering with a demo buffer.

### Phase 2: Wire Up Editor [done]
HelideApp struct owns Editor, Compositor, Terminal<GpuBackend>. Constructs Handlers manually (dummy LSP channels + real word_index). Loads theme and extracts default colors. Compositor renders to buffer, terminal diffs, backend flushes to GPU.

### Phase 3: Event Loop + Input [done]
winit ApplicationHandler with tokio runtime on main thread. Key/mouse/scroll events converted from winit to helix Event types. Focus, resize, close handled. Clean shutdown drops wgpu resources before winit exits.

### Phase 4: Input Mapping [done]
~30 key codes mapped (arrows, F-keys, modifiers, special keys). Mouse press/release/scroll with cell coordinate conversion. Shift modifier stripped for already-capitalized characters.

### Phase 5: Decoration Rendering [done]
Third render pass for underlines (line, double, curl, dotted, dashed), strikethrough, and cursor overlay. Underline color from cell or fg fallback. Reuses bg pipeline (colored rects).

### Phase 6: Polish [done]
CLI file opening (`helide file.rs`). Runtime auto-discovery: checks ./helix/runtime, ~/.config/helix/runtime, next to `hx` binary. DPI-aware font sizing. Non-sRGB surface for correct colors. Theme-aware default colors.
