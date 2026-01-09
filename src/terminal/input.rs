use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use smithay::{
    backend::input::ButtonState,
    input::keyboard::Keysym,
};
use std::time::Duration;

/// Terminal input handler
pub struct TerminalInput {
    term_width: u32,
    term_height: u32,
    pixel_width: u32,
    pixel_height: u32,
}

impl TerminalInput {
    pub fn new(term_width: u32, term_height: u32, pixel_width: u32, pixel_height: u32) -> Self {
        Self {
            term_width,
            term_height,
            pixel_width,
            pixel_height,
        }
    }

    pub fn update_dimensions(
        &mut self,
        term_width: u32,
        term_height: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) {
        self.term_width = term_width;
        self.term_height = term_height;
        self.pixel_width = pixel_width;
        self.pixel_height = pixel_height;
    }

    /// Enable mouse capture in terminal
    pub fn enable_mouse_capture() -> Result<()> {
        use crossterm::execute;
        use crossterm::event::{
            EnableMouseCapture, EnableBracketedPaste,
            PushKeyboardEnhancementFlags, KeyboardEnhancementFlags,
        };
        use std::io::stdout;

        crossterm::terminal::enable_raw_mode()?;

        // Check if enhanced keyboard mode is supported
        let supports_enhanced = crossterm::terminal::supports_keyboard_enhancement()
            .unwrap_or(false);
        tracing::info!("Terminal supports enhanced keyboard: {}", supports_enhanced);

        let mut stdout = stdout();
        execute!(stdout, EnableMouseCapture, EnableBracketedPaste)?;

        if supports_enhanced {
            // Enable Kitty keyboard protocol for proper key release events
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                ),
            )?;
        }

        Ok(())
    }

    /// Disable mouse capture
    pub fn disable_mouse_capture() -> Result<()> {
        use crossterm::execute;
        use crossterm::event::{
            DisableMouseCapture, DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
        };
        use std::io::stdout;

        let supports_enhanced = crossterm::terminal::supports_keyboard_enhancement()
            .unwrap_or(false);

        let mut stdout = stdout();
        if supports_enhanced {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        execute!(stdout, DisableMouseCapture, DisableBracketedPaste)?;
        crossterm::terminal::disable_raw_mode()?;
        Ok(())
    }

    /// Poll for terminal input events (non-blocking)
    pub fn poll_event(timeout: Duration) -> Result<Option<Event>> {
        if event::poll(timeout)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }

    /// Convert terminal cell coordinates to pixel coordinates
    pub fn cell_to_pixel(&self, col: u16, row: u16) -> (f64, f64) {
        let cell_width = self.pixel_width as f64 / self.term_width as f64;
        let cell_height = self.pixel_height as f64 / self.term_height as f64;

        let x = col as f64 * cell_width + cell_width / 2.0;
        let y = row as f64 * cell_height + cell_height / 2.0;

        (x, y)
    }

    /// Convert crossterm key code to xkbcommon keysym
    pub fn keycode_to_keysym(key: KeyCode) -> Option<Keysym> {
        Some(match key {
            KeyCode::Char(c) => {
                // For ASCII characters, keysym is often the same as the char code
                let code = c as u32;
                if code < 128 {
                    Keysym::new(code)
                } else {
                    // Unicode characters: use Unicode keysym range
                    Keysym::new(0x01000000 + code)
                }
            }
            KeyCode::Enter => Keysym::new(0xff0d),        // XKB_KEY_Return
            KeyCode::Tab => Keysym::new(0xff09),          // XKB_KEY_Tab
            KeyCode::Backspace => Keysym::new(0xff08),    // XKB_KEY_BackSpace
            KeyCode::Esc => Keysym::new(0xff1b),          // XKB_KEY_Escape
            KeyCode::Left => Keysym::new(0xff51),         // XKB_KEY_Left
            KeyCode::Right => Keysym::new(0xff53),        // XKB_KEY_Right
            KeyCode::Up => Keysym::new(0xff52),           // XKB_KEY_Up
            KeyCode::Down => Keysym::new(0xff54),         // XKB_KEY_Down
            KeyCode::Home => Keysym::new(0xff50),         // XKB_KEY_Home
            KeyCode::End => Keysym::new(0xff57),          // XKB_KEY_End
            KeyCode::PageUp => Keysym::new(0xff55),       // XKB_KEY_Page_Up
            KeyCode::PageDown => Keysym::new(0xff56),     // XKB_KEY_Page_Down
            KeyCode::Insert => Keysym::new(0xff63),       // XKB_KEY_Insert
            KeyCode::Delete => Keysym::new(0xffff),       // XKB_KEY_Delete
            KeyCode::F(1) => Keysym::new(0xffbe),         // XKB_KEY_F1
            KeyCode::F(2) => Keysym::new(0xffbf),
            KeyCode::F(3) => Keysym::new(0xffc0),
            KeyCode::F(4) => Keysym::new(0xffc1),
            KeyCode::F(5) => Keysym::new(0xffc2),
            KeyCode::F(6) => Keysym::new(0xffc3),
            KeyCode::F(7) => Keysym::new(0xffc4),
            KeyCode::F(8) => Keysym::new(0xffc5),
            KeyCode::F(9) => Keysym::new(0xffc6),
            KeyCode::F(10) => Keysym::new(0xffc7),
            KeyCode::F(11) => Keysym::new(0xffc8),
            KeyCode::F(12) => Keysym::new(0xffc9),
            KeyCode::F(n) => Keysym::new(0xffbe + (n as u32 - 1)),
            _ => return None,
        })
    }

    /// Convert mouse button to Wayland button code
    pub fn mouse_button_to_code(button: MouseButton) -> u32 {
        match button {
            MouseButton::Left => 0x110,   // BTN_LEFT
            MouseButton::Right => 0x111,  // BTN_RIGHT
            MouseButton::Middle => 0x112, // BTN_MIDDLE
        }
    }
}

/// Input event types for the compositor
pub enum WaylandInputEvent {
    PointerMotion {
        x: f64,
        y: f64,
        time: u32,
    },
    PointerButton {
        button: u32,
        state: ButtonState,
        time: u32,
    },
    PointerAxis {
        horizontal: f64,
        vertical: f64,
        time: u32,
    },
    KeyboardKey {
        keysym: Keysym,
        state: KeyState,
        time: u32,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Quit,
}

#[derive(Clone, Copy, Debug)]
pub enum KeyState {
    Pressed,
    Released,
}

impl TerminalInput {
    /// Convert a crossterm event to a Wayland input event
    pub fn translate_event(&self, event: Event) -> Option<WaylandInputEvent> {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u32;

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) => {
                Some(WaylandInputEvent::Quit)
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) => {
                Some(WaylandInputEvent::Quit)
            }

            Event::Key(KeyEvent { code, kind, .. }) => {
                let keysym = Self::keycode_to_keysym(code)?;
                let state = match kind {
                    event::KeyEventKind::Press | event::KeyEventKind::Repeat => KeyState::Pressed,
                    event::KeyEventKind::Release => KeyState::Released,
                };
                Some(WaylandInputEvent::KeyboardKey { keysym, state, time })
            }

            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: _,
            }) => {
                let (x, y) = self.cell_to_pixel(column, row);

                match kind {
                    MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                        Some(WaylandInputEvent::PointerMotion { x, y, time })
                    }
                    MouseEventKind::Down(button) => {
                        Some(WaylandInputEvent::PointerButton {
                            button: Self::mouse_button_to_code(button),
                            state: ButtonState::Pressed,
                            time,
                        })
                    }
                    MouseEventKind::Up(button) => {
                        Some(WaylandInputEvent::PointerButton {
                            button: Self::mouse_button_to_code(button),
                            state: ButtonState::Released,
                            time,
                        })
                    }
                    MouseEventKind::ScrollDown => {
                        Some(WaylandInputEvent::PointerAxis {
                            horizontal: 0.0,
                            vertical: 15.0,
                            time,
                        })
                    }
                    MouseEventKind::ScrollUp => {
                        Some(WaylandInputEvent::PointerAxis {
                            horizontal: 0.0,
                            vertical: -15.0,
                            time,
                        })
                    }
                    MouseEventKind::ScrollLeft => {
                        Some(WaylandInputEvent::PointerAxis {
                            horizontal: -15.0,
                            vertical: 0.0,
                            time,
                        })
                    }
                    MouseEventKind::ScrollRight => {
                        Some(WaylandInputEvent::PointerAxis {
                            horizontal: 15.0,
                            vertical: 0.0,
                            time,
                        })
                    }
                }
            }

            Event::Resize(cols, rows) => {
                // Recalculate pixel dimensions
                let (pixel_width, pixel_height) =
                    super::KittyGraphics::query_terminal_size_pixels().unwrap_or((
                        cols as u32 * 10,
                        rows as u32 * 20,
                    ));
                Some(WaylandInputEvent::Resize {
                    width: pixel_width,
                    height: pixel_height,
                })
            }

            _ => None,
        }
    }
}
