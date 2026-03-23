use helix_view::input::{Event, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::KeyCode;
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{Key, NamedKey};

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

/// Convert winit scroll event to helix mouse scroll event.
pub fn convert_scroll(
    delta: MouseScrollDelta,
    cursor_pos: (f64, f64),
    cell_size: (f32, f32),
    modifiers: &winit::event::Modifiers,
) -> Option<Event> {
    let (dx, dy) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(pos) => (pos.x as f32 / 20.0, pos.y as f32 / 20.0),
    };

    let kind = if dy.abs() > dx.abs() {
        if dy > 0.0 {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        }
    } else if dx > 0.0 {
        MouseEventKind::ScrollRight
    } else {
        MouseEventKind::ScrollLeft
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
