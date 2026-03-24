use winit::event::{ElementState, Modifiers};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

/// Encode a winit key event as terminal escape sequence bytes.
pub fn encode_key(
    key_event: &winit::event::KeyEvent,
    modifiers: &Modifiers,
) -> Option<Vec<u8>> {
    // Only handle key press events
    if key_event.state != ElementState::Pressed {
        return None;
    }

    let ctrl = modifiers.state().control_key();
    let alt = modifiers.state().alt_key();
    let shift = modifiers.state().shift_key();

    // Handle named keys
    match &key_event.logical_key {
        Key::Named(named) => {
            let bytes = match named {
                NamedKey::Enter => Some(b"\r".to_vec()),
                NamedKey::Backspace => {
                    if alt {
                        Some(b"\x1b\x7f".to_vec())
                    } else {
                        Some(vec![0x7f])
                    }
                }
                NamedKey::Tab => {
                    if shift {
                        Some(b"\x1b[Z".to_vec()) // reverse tab
                    } else {
                        Some(b"\t".to_vec())
                    }
                }
                NamedKey::Escape => Some(b"\x1b".to_vec()),
                NamedKey::ArrowUp => {
                    if ctrl {
                        Some(b"\x1b[1;5A".to_vec())
                    } else if alt {
                        Some(b"\x1b[1;3A".to_vec())
                    } else if shift {
                        Some(b"\x1b[1;2A".to_vec())
                    } else {
                        Some(b"\x1b[A".to_vec())
                    }
                }
                NamedKey::ArrowDown => {
                    if ctrl {
                        Some(b"\x1b[1;5B".to_vec())
                    } else if alt {
                        Some(b"\x1b[1;3B".to_vec())
                    } else if shift {
                        Some(b"\x1b[1;2B".to_vec())
                    } else {
                        Some(b"\x1b[B".to_vec())
                    }
                }
                NamedKey::ArrowRight => {
                    if ctrl {
                        Some(b"\x1b[1;5C".to_vec())
                    } else if alt {
                        Some(b"\x1b[1;3C".to_vec())
                    } else if shift {
                        Some(b"\x1b[1;2C".to_vec())
                    } else {
                        Some(b"\x1b[C".to_vec())
                    }
                }
                NamedKey::ArrowLeft => {
                    if ctrl {
                        Some(b"\x1b[1;5D".to_vec())
                    } else if alt {
                        Some(b"\x1b[1;3D".to_vec())
                    } else if shift {
                        Some(b"\x1b[1;2D".to_vec())
                    } else {
                        Some(b"\x1b[D".to_vec())
                    }
                }
                NamedKey::Home => {
                    if ctrl {
                        Some(b"\x1b[1;5H".to_vec())
                    } else {
                        Some(b"\x1b[H".to_vec())
                    }
                }
                NamedKey::End => {
                    if ctrl {
                        Some(b"\x1b[1;5F".to_vec())
                    } else {
                        Some(b"\x1b[F".to_vec())
                    }
                }
                NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
                NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
                NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
                NamedKey::Delete => {
                    if alt {
                        Some(b"\x1b[3;3~".to_vec())
                    } else if ctrl {
                        Some(b"\x1b[3;5~".to_vec())
                    } else {
                        Some(b"\x1b[3~".to_vec())
                    }
                }
                NamedKey::F1 => Some(b"\x1bOP".to_vec()),
                NamedKey::F2 => Some(b"\x1bOQ".to_vec()),
                NamedKey::F3 => Some(b"\x1bOR".to_vec()),
                NamedKey::F4 => Some(b"\x1bOS".to_vec()),
                NamedKey::F5 => Some(b"\x1b[15~".to_vec()),
                NamedKey::F6 => Some(b"\x1b[17~".to_vec()),
                NamedKey::F7 => Some(b"\x1b[18~".to_vec()),
                NamedKey::F8 => Some(b"\x1b[19~".to_vec()),
                NamedKey::F9 => Some(b"\x1b[20~".to_vec()),
                NamedKey::F10 => Some(b"\x1b[21~".to_vec()),
                NamedKey::F11 => Some(b"\x1b[23~".to_vec()),
                NamedKey::F12 => Some(b"\x1b[24~".to_vec()),
                NamedKey::Space => {
                    if ctrl {
                        // Ctrl+Space sends NUL
                        Some(vec![0x00])
                    } else {
                        Some(b" ".to_vec())
                    }
                }
                _ => None,
            };
            return bytes;
        }
        Key::Character(text) => {
            // Ctrl+letter → control code (0x01-0x1a)
            if ctrl && !alt && !shift {
                let ch = text.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    let code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![code]);
                }
                // Common Ctrl combos
                match ch {
                    '[' | '3' => return Some(vec![0x1b]), // Ctrl+[ = ESC
                    '\\' | '4' => return Some(vec![0x1c]),
                    ']' | '5' => return Some(vec![0x1d]),
                    '^' | '6' => return Some(vec![0x1e]),
                    '_' | '7' => return Some(vec![0x1f]),
                    '2' | '@' => return Some(vec![0x00]), // Ctrl+2 = NUL
                    '8' => return Some(vec![0x7f]),        // Ctrl+8 = DEL
                    _ => {}
                }
            }

            // Alt+key → ESC prefix
            if alt && !ctrl {
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(text.as_bytes());
                return Some(bytes);
            }

            // Plain text input
            if !text.is_empty() {
                return Some(text.as_bytes().to_vec());
            }
        }
        Key::Unidentified(_) => {
            // Try physical key for Ctrl combinations
            if ctrl {
                if let PhysicalKey::Code(keycode) = key_event.physical_key {
                    let code = match keycode {
                        KeyCode::KeyA => Some(1),
                        KeyCode::KeyB => Some(2),
                        KeyCode::KeyC => Some(3),
                        KeyCode::KeyD => Some(4),
                        KeyCode::KeyE => Some(5),
                        KeyCode::KeyF => Some(6),
                        KeyCode::KeyG => Some(7),
                        KeyCode::KeyH => Some(8),
                        KeyCode::KeyI => Some(9),
                        KeyCode::KeyJ => Some(10),
                        KeyCode::KeyK => Some(11),
                        KeyCode::KeyL => Some(12),
                        KeyCode::KeyM => Some(13),
                        KeyCode::KeyN => Some(14),
                        KeyCode::KeyO => Some(15),
                        KeyCode::KeyP => Some(16),
                        KeyCode::KeyQ => Some(17),
                        KeyCode::KeyR => Some(18),
                        KeyCode::KeyS => Some(19),
                        KeyCode::KeyT => Some(20),
                        KeyCode::KeyU => Some(21),
                        KeyCode::KeyV => Some(22),
                        KeyCode::KeyW => Some(23),
                        KeyCode::KeyX => Some(24),
                        KeyCode::KeyY => Some(25),
                        KeyCode::KeyZ => Some(26),
                        _ => None,
                    };
                    if let Some(c) = code {
                        return Some(vec![c]);
                    }
                }
            }
        }
        Key::Dead(_) => {}
    }

    None
}
