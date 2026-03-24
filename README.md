# Helide

Experimental, vibe-coded GUI frontend for the [Helix](https://helix-editor.com) text editor on macOS.

This is a proof-of-concept — a native GPU-accelerated window that replaces Helix's terminal backend with direct rendering via wgpu. Unlike [Neovide](https://neovide.dev) (which talks to Neovim over msgpack-RPC), Helide integrates Helix in-process.

Uses a [forked Helix](https://github.com/polachok/helix/tree/for-helide) with a small patch to make completion handlers public ([`c406e175`](https://github.com/polachok/helix/commit/c406e175c365a1df6e0f48f1a7ecb4a872e696ab)).

## Features

- GPU-accelerated rendering via wgpu (Metal on macOS)
- Full Helix editor: keybindings, syntax highlighting, themes, LSP, file picker
- Embedded terminal emulator (alacritty_terminal) with Ctrl+` toggle
- Native macOS: transparent titlebar, menu bar, Open With, drag-and-drop, Open Recent
- .app bundle with DMG packaging

## Building

Requires Rust 1.87+.

```sh
git clone --recursive https://github.com/polachok/helide
cd helide

# Development
cargo run -- file.rs

# Release + install to /Applications
INSTALL=true ./macos-builder/run

# Release + DMG
GENERATE_DMG=true ./macos-builder/run
```

## Status

Proof-of-concept. Things will break. Not ready for daily use.

## License

[MPL-2.0](LICENSE)
