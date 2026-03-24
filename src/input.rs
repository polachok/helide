use helix_view::input::{Event, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::KeyCode;
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey};

/// Detect Ctrl+` (toggle terminal keybind).
pub fn is_toggle_terminal(event: &winit::event::KeyEvent, modifiers: &winit::event::Modifiers) -> bool {
    if event.state != ElementState::Pressed {
        return false;
    }
    let ctrl = modifiers.state().control_key();
    if !ctrl {
        return false;
    }
    matches!(event.physical_key, PhysicalKey::Code(WinitKeyCode::Backquote))
}

/// Convert a winit key event to a helix key event.
pub fn convert_key_event(
    event: &winit::event::KeyEvent,
    modifiers: &winit::event::Modifiers,
) -> Option<Event> {
    if event.state != ElementState::Pressed {
        return None;
    }

    let code = match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::Insert => KeyCode::Insert,
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::CapsLock => KeyCode::CapsLock,
            NamedKey::ScrollLock => KeyCode::ScrollLock,
            NamedKey::NumLock => KeyCode::NumLock,
            NamedKey::PrintScreen => KeyCode::PrintScreen,
            NamedKey::Pause => KeyCode::Pause,
            NamedKey::ContextMenu => KeyCode::Menu,
            NamedKey::F1 => KeyCode::F(1),
            NamedKey::F2 => KeyCode::F(2),
            NamedKey::F3 => KeyCode::F(3),
            NamedKey::F4 => KeyCode::F(4),
            NamedKey::F5 => KeyCode::F(5),
            NamedKey::F6 => KeyCode::F(6),
            NamedKey::F7 => KeyCode::F(7),
            NamedKey::F8 => KeyCode::F(8),
            NamedKey::F9 => KeyCode::F(9),
            NamedKey::F10 => KeyCode::F(10),
            NamedKey::F11 => KeyCode::F(11),
            NamedKey::F12 => KeyCode::F(12),
            _ => return None,
        },
        Key::Character(s) => {
            let ch = s.chars().next()?;
            KeyCode::Char(ch)
        }
        _ => return None,
    };

    let state = modifiers.state();
    let mut mods = KeyModifiers::NONE;
    if state.shift_key() {
        mods.insert(KeyModifiers::SHIFT);
    }
    if state.control_key() {
        mods.insert(KeyModifiers::CONTROL);
    }
    if state.alt_key() {
        mods.insert(KeyModifiers::ALT);
    }
    if state.super_key() {
        mods.insert(KeyModifiers::SUPER);
    }

    // For characters, winit already applies shift (e.g. 'A' not 'a'),
    // so strip the shift modifier for printable chars to match helix expectations
    if let KeyCode::Char(ch) = code {
        if ch.is_alphabetic() && mods.contains(KeyModifiers::SHIFT) && !ch.is_lowercase() {
            mods.remove(KeyModifiers::SHIFT);
        }
    }

    Some(Event::Key(KeyEvent {
        code,
        modifiers: mods,
    }))
}

/// Convert winit mouse button to helix mouse button.
fn convert_mouse_button(button: winit::event::MouseButton) -> Option<MouseButton> {
    match button {
        winit::event::MouseButton::Left => Some(MouseButton::Left),
        winit::event::MouseButton::Right => Some(MouseButton::Right),
        winit::event::MouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

/// Convert a winit mouse press/release to a helix mouse event.
pub fn convert_mouse_press(
    state: ElementState,
    button: winit::event::MouseButton,
    cursor_pos: (f64, f64),
    cell_size: (f32, f32),
    modifiers: &winit::event::Modifiers,
) -> Option<Event> {
    let hx_button = convert_mouse_button(button)?;
    let kind = match state {
        ElementState::Pressed => MouseEventKind::Down(hx_button),
        ElementState::Released => MouseEventKind::Up(hx_button),
    };

    let col = (cursor_pos.0 as f32 / cell_size.0) as u16;
    let row = (cursor_pos.1 as f32 / cell_size.1) as u16;

    let mod_state = modifiers.state();
    let mut mods = KeyModifiers::NONE;
    if mod_state.shift_key() {
        mods.insert(KeyModifiers::SHIFT);
    }
    if mod_state.control_key() {
        mods.insert(KeyModifiers::CONTROL);
    }
    if mod_state.alt_key() {
        mods.insert(KeyModifiers::ALT);
    }

    Some(Event::Mouse(MouseEvent {
        kind,
        column: col,
        row,
        modifiers: mods,
    }))
}

/// Scroll accumulator for smooth pixel-based scrolling.
/// Accumulates pixel deltas and only emits scroll events per cell height.
pub struct ScrollAccumulator {
    dx: f32,
    dy: f32,
}

impl ScrollAccumulator {
    pub fn new() -> Self {
        ScrollAccumulator { dx: 0.0, dy: 0.0 }
    }

    /// Accumulate a scroll delta. Returns scroll events to emit (may be 0 or more).
    pub fn accumulate(
        &mut self,
        delta: MouseScrollDelta,
        cursor_pos: (f64, f64),
        cell_size: (f32, f32),
        modifiers: &winit::event::Modifiers,
    ) -> Vec<Event> {
        match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                // Line deltas: emit directly, one event per line
                let mut events = Vec::new();
                let count_y = y.abs().ceil() as usize;
                let count_x = x.abs().ceil() as usize;

                let kind_y = if y > 0.0 {
                    Some(MouseEventKind::ScrollUp)
                } else if y < 0.0 {
                    Some(MouseEventKind::ScrollDown)
                } else {
                    None
                };
                let kind_x = if x > 0.0 {
                    Some(MouseEventKind::ScrollRight)
                } else if x < 0.0 {
                    Some(MouseEventKind::ScrollLeft)
                } else {
                    None
                };

                let (col, row, mods) = event_params(cursor_pos, cell_size, modifiers);

                if let Some(kind) = kind_y {
                    for _ in 0..count_y {
                        events.push(Event::Mouse(MouseEvent {
                            kind,
                            column: col,
                            row,
                            modifiers: mods,
                        }));
                    }
                }
                if let Some(kind) = kind_x {
                    for _ in 0..count_x {
                        events.push(Event::Mouse(MouseEvent {
                            kind,
                            column: col,
                            row,
                            modifiers: mods,
                        }));
                    }
                }
                events
            }
            MouseScrollDelta::PixelDelta(pos) => {
                self.dx += pos.x as f32;
                self.dy += pos.y as f32;

                let mut events = Vec::new();
                let threshold = cell_size.1; // one cell height per scroll event

                let (col, row, mods) = event_params(cursor_pos, cell_size, modifiers);

                while self.dy.abs() >= threshold {
                    let kind = if self.dy > 0.0 {
                        MouseEventKind::ScrollUp
                    } else {
                        MouseEventKind::ScrollDown
                    };
                    events.push(Event::Mouse(MouseEvent {
                        kind,
                        column: col,
                        row,
                        modifiers: mods,
                    }));
                    self.dy -= threshold.copysign(self.dy);
                }

                while self.dx.abs() >= threshold {
                    let kind = if self.dx > 0.0 {
                        MouseEventKind::ScrollRight
                    } else {
                        MouseEventKind::ScrollLeft
                    };
                    events.push(Event::Mouse(MouseEvent {
                        kind,
                        column: col,
                        row,
                        modifiers: mods,
                    }));
                    self.dx -= threshold.copysign(self.dx);
                }

                events
            }
        }
    }
}

fn event_params(
    cursor_pos: (f64, f64),
    cell_size: (f32, f32),
    modifiers: &winit::event::Modifiers,
) -> (u16, u16, KeyModifiers) {
    let col = (cursor_pos.0 as f32 / cell_size.0) as u16;
    let row = (cursor_pos.1 as f32 / cell_size.1) as u16;

    let mod_state = modifiers.state();
    let mut mods = KeyModifiers::NONE;
    if mod_state.shift_key() {
        mods.insert(KeyModifiers::SHIFT);
    }
    if mod_state.control_key() {
        mods.insert(KeyModifiers::CONTROL);
    }
    if mod_state.alt_key() {
        mods.insert(KeyModifiers::ALT);
    }
    (col, row, mods)
}
