mod compositor;
mod terminal;

use anyhow::{anyhow, Result};
use compositor::{ClientState, TermuiState};
use smithay::{
    backend::input::Axis,
    input::{
        keyboard::Keycode,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::{
        calloop::{
            channel::{self},
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, Mode, PostAction,
        },
        wayland_server::{Display, ListeningSocket},
    },
    utils::{Point, SERIAL_COUNTER},
};
use std::{
    env,
    process::Command,
    sync::Arc,
    time::Duration,
};
use terminal::{KittyGraphics, TerminalInput, WaylandInputEvent};
use tracing::{error, info};

fn main() -> Result<()> {
    // Redirect logging to file so it doesn't interfere with terminal graphics
    let log_file = std::fs::File::create("/tmp/termui.log")
        .map_err(|e| anyhow!("Failed to create log file: {}", e))?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .with_writer(log_file)
        .init();

    // Get command to run from args
    let args: Vec<String> = env::args().skip(1).collect();

    // Check for --headless flag (for testing)
    let headless = args.first().map(|s| s == "--headless").unwrap_or(false);
    let args: Vec<String> = if headless { args.into_iter().skip(1).collect() } else { args };

    if args.is_empty() {
        eprintln!("Usage: termui [--headless] <command> [args...]");
        eprintln!();
        eprintln!("Run a graphical Wayland application in the terminal using Kitty graphics protocol.");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --headless  Run without terminal graphics (for testing)");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  termui foot");
        eprintln!("  termui gtk4-demo");
        std::process::exit(1);
    }

    info!("Headless mode: {}", headless);

    // Get terminal dimensions (use defaults in headless mode)
    let (term_cols, term_rows) = if headless {
        (80, 24)
    } else {
        KittyGraphics::query_terminal_size_chars()?
    };
    let (pixel_width, pixel_height) = if headless {
        (800, 600)
    } else {
        KittyGraphics::query_terminal_size_pixels()?
    };

    // Scale factor for the virtual display (higher = larger UI elements)
    // Use 1 for 1:1 pixel mapping, 2-4 for HiDPI-like scaling
    let scale_factor: u32 = 4;
    let virtual_width = pixel_width / scale_factor;
    let virtual_height = pixel_height / scale_factor;

    info!(
        "Terminal size: {}x{} chars, {}x{} pixels, virtual: {}x{} (scale {})",
        term_cols, term_rows, pixel_width, pixel_height, virtual_width, virtual_height, scale_factor
    );

    // Create event loop
    let mut event_loop: EventLoop<TermuiState> =
        EventLoop::try_new().map_err(|e| anyhow!("Failed to create event loop: {}", e))?;

    // Create Wayland display
    let display: Display<TermuiState> = Display::new()
        .map_err(|e| anyhow!("Failed to create display: {}", e))?;

    // Create compositor state with virtual (scaled) dimensions
    let mut state = TermuiState::new(
        &display,
        event_loop.get_signal(),
        virtual_width,
        virtual_height,
    );

    // Use XDG_RUNTIME_DIR or create our own in /tmp
    let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let tmp_dir = format!("/tmp/termui-{}", std::process::id());
        std::fs::create_dir_all(&tmp_dir).ok();
        tmp_dir
    });

    // Ensure directory exists and is writable
    if std::fs::metadata(&runtime_dir).is_err() {
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|e| anyhow!("Failed to create runtime dir {}: {}", runtime_dir, e))?;
    }
    env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
    info!("XDG_RUNTIME_DIR: {}", runtime_dir);

    // Set up Wayland socket
    let socket = ListeningSocket::bind_auto("termui", 1..=32)
        .map_err(|e| anyhow!("Failed to create Wayland socket in {}: {}", runtime_dir, e))?;
    let socket_name = socket.socket_name().unwrap().to_string_lossy().to_string();

    info!("Wayland socket: {}/{}", runtime_dir, socket_name);

    // Add socket to event loop
    event_loop
        .handle()
        .insert_source(
            Generic::new(socket, Interest::READ, Mode::Level),
            move |_, socket, state| {
                match socket.accept() {
                    Ok(Some(client_stream)) => {
                        info!("New client connecting...");
                        let client = state
                            .display_handle
                            .insert_client(
                                client_stream,
                                Arc::new(ClientState {
                                    compositor_state:
                                        smithay::wayland::compositor::CompositorClientState::default(),
                                }),
                            );
                        match client {
                            Ok(_) => {
                                info!("Client connected successfully");
                                // Flush to send globals to new client
                                let _ = state.display_handle.flush_clients();
                            }
                            Err(e) => {
                                error!("Failed to insert client: {:?}", e);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Error accepting client: {:?}", e);
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("Failed to add socket to event loop: {:?}", e))?;

    // Add display to event loop
    event_loop
        .handle()
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: we don't drop the display
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                    display.get_mut().flush_clients().unwrap();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("Failed to add display to event loop: {:?}", e))?;

    // Set up terminal input channel
    let (input_tx, input_rx) = channel::channel::<WaylandInputEvent>();

    event_loop
        .handle()
        .insert_source(input_rx, |event, _, state| {
            if let channel::Event::Msg(input_event) = event {
                tracing::debug!("Input event received: {:?}", std::mem::discriminant(&input_event));
                handle_input_event(state, input_event);
                // Flush display to ensure events are sent to client immediately
                if let Err(e) = state.display_handle.flush_clients() {
                    tracing::error!("Failed to flush display: {:?}", e);
                }
            }
        })
        .map_err(|e| anyhow!("Failed to add input channel to event loop: {:?}", e))?;

    // Frame timer for rendering (target ~30 fps)
    let frame_timer = Timer::from_duration(Duration::from_millis(33));

    let mut kitty = KittyGraphics::new();

    event_loop
        .handle()
        .insert_source(frame_timer, move |_, _, state| {
            // Check for pending frame and render
            if let Some(frame) = state.pending_frame.lock().unwrap().take() {
                if let Err(e) = kitty.display_frame(frame.width, frame.height, &frame.data) {
                    error!("Failed to render frame: {:?}", e);
                }
            }
            TimeoutAction::ToDuration(Duration::from_millis(33))
        })
        .map_err(|e| anyhow!("Failed to add frame timer to event loop: {:?}", e))?;

    // Set up terminal (skip in headless mode)
    let kitty_setup = KittyGraphics::new();
    if !headless {
        kitty_setup.setup_terminal()?;
        TerminalInput::enable_mouse_capture()?;
    }

    // Spawn input handling thread (skip in headless mode)
    let _input_thread = if !headless {
        let input_tx = input_tx.clone();
        Some(std::thread::spawn(move || {
            // Use virtual dimensions for input scaling
            let mut term_input = TerminalInput::new(
                term_cols as u32,
                term_rows as u32,
                virtual_width,
                virtual_height,
            );

            loop {
                match TerminalInput::poll_event(Duration::from_millis(10)) {
                    Ok(Some(event)) => {
                        if let Some(input_event) = term_input.translate_event(event) {
                            // Update dimensions on resize
                            if let WaylandInputEvent::Resize { width, height } = &input_event {
                                let (cols, rows) =
                                    KittyGraphics::query_terminal_size_chars().unwrap_or((80, 24));
                                term_input.update_dimensions(
                                    cols as u32,
                                    rows as u32,
                                    *width,
                                    *height,
                                );
                            }

                            let is_quit = matches!(input_event, WaylandInputEvent::Quit);
                            if input_tx.send(input_event).is_err() || is_quit {
                                break;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Input error: {:?}", e);
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    // Ensure display is ready before spawning client
    info!("Display ready, spawning client...");

    // Spawn the child process
    let _child = Command::new(&args[0])
        .args(&args[1..])
        .env("WAYLAND_DISPLAY", &socket_name)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CURRENT_DESKTOP", "termui")
        .env("GDK_BACKEND", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        .env("SDL_VIDEODRIVER", "wayland")
        .env("MOZ_ENABLE_WAYLAND", "1")
        .env("_JAVA_AWT_WM_NONREPARENTING", "1")
        // Force software rendering (we only support wl_shm)
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .env("WLR_RENDERER", "pixman")
        .env("GALLIUM_DRIVER", "llvmpipe")
        .env("__GLX_VENDOR_LIBRARY_NAME", "mesa")
        .env("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe")
        // Disable things that might cause issues
        .env_remove("DISPLAY")
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {}: {}", args[0], e))?;

    info!("Spawned child process");

    // Run the event loop
    while state.running {
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .map_err(|e| anyhow!("Event loop error: {}", e))?;
    }

    // Cleanup
    info!("Shutting down...");
    if !headless {
        TerminalInput::disable_mouse_capture()?;
        kitty_setup.restore_terminal()?;
    }

    Ok(())
}

fn handle_input_event(state: &mut TermuiState, event: WaylandInputEvent) {
    let serial = SERIAL_COUNTER.next_serial();

    match event {
        WaylandInputEvent::PointerMotion { x, y, time } => {
            state.pointer_location = Point::from((x, y));

            let pointer = state.seat.get_pointer().unwrap();

            if let Some((surface, offset)) = state.surface_under_pointer() {
                pointer.motion(
                    state,
                    Some((surface, offset)),
                    &MotionEvent {
                        location: state.pointer_location,
                        serial,
                        time,
                    },
                );
            }
            pointer.frame(state);
        }

        WaylandInputEvent::PointerButton { button, state: btn_state, time } => {
            let pointer = state.seat.get_pointer().unwrap();
            pointer.button(
                state,
                &ButtonEvent {
                    serial,
                    time,
                    button,
                    state: btn_state,
                },
            );
            pointer.frame(state);
        }

        WaylandInputEvent::PointerAxis { horizontal, vertical, time } => {
            let pointer = state.seat.get_pointer().unwrap();

            let mut frame = AxisFrame::new(time);
            if vertical != 0.0 {
                frame = frame.value(Axis::Vertical, vertical);
            }
            if horizontal != 0.0 {
                frame = frame.value(Axis::Horizontal, horizontal);
            }

            pointer.axis(state, frame);
            pointer.frame(state);
        }

        WaylandInputEvent::KeyboardKey { keysym, state: key_state, time } => {
            let keyboard = state.seat.get_keyboard().unwrap();

            // Convert keysym to keycode
            let keycode = keysym_to_keycode(keysym);

            let pressed = matches!(key_state, terminal::KeyState::Pressed);
            tracing::info!(
                "Key: keysym=0x{:x} ({}) -> keycode={}, state={}",
                keysym.raw(),
                char::from_u32(keysym.raw()).unwrap_or('?'),
                keycode.raw(),
                if pressed { "PRESS" } else { "RELEASE" }
            );
            let key_state = if pressed {
                smithay::backend::input::KeyState::Pressed
            } else {
                smithay::backend::input::KeyState::Released
            };

            keyboard.input::<(), _>(
                state,
                keycode,
                key_state,
                serial,
                time,
                |_, _, _| {
                    smithay::input::keyboard::FilterResult::Forward
                },
            );
        }

        WaylandInputEvent::Resize { width, height } => {
            state.resize_output(width, height);
        }

        WaylandInputEvent::Quit => {
            state.running = false;
            state.loop_signal.stop();
        }
    }
}

/// Convert keysym to XKB keycode (evdev + 8 offset)
/// XKB keycodes are Linux evdev keycodes + 8
fn keysym_to_keycode(keysym: smithay::input::keyboard::Keysym) -> Keycode {
    let raw = keysym.raw();

    // evdev keycodes - we'll add 8 at the end for XKB
    let evdev_code = match raw {
        // Function keys
        0xff1b => 1,         // Escape -> KEY_ESC
        0xff0d => 28,        // Return -> KEY_ENTER
        0xff09 => 15,        // Tab -> KEY_TAB
        0xff08 => 14,        // BackSpace -> KEY_BACKSPACE
        0x20 => 57,          // Space -> KEY_SPACE

        // Arrow keys
        0xff51 => 105,       // Left -> KEY_LEFT
        0xff53 => 106,       // Right -> KEY_RIGHT
        0xff52 => 103,       // Up -> KEY_UP
        0xff54 => 108,       // Down -> KEY_DOWN

        // Navigation
        0xff50 => 102,       // Home -> KEY_HOME
        0xff57 => 107,       // End -> KEY_END
        0xff55 => 104,       // Page_Up -> KEY_PAGEUP
        0xff56 => 109,       // Page_Down -> KEY_PAGEDOWN
        0xff63 => 110,       // Insert -> KEY_INSERT
        0xffff => 111,       // Delete -> KEY_DELETE

        // Letters (US QWERTY layout keycodes)
        c if c < 128 => {
            match c as u8 as char {
                // Number row
                '1' | '!' => 2,
                '2' | '@' => 3,
                '3' | '#' => 4,
                '4' | '$' => 5,
                '5' | '%' => 6,
                '6' | '^' => 7,
                '7' | '&' => 8,
                '8' | '*' => 9,
                '9' | '(' => 10,
                '0' | ')' => 11,
                '-' | '_' => 12,
                '=' | '+' => 13,

                // Top row: QWERTYUIOP
                'q' | 'Q' => 16,
                'w' | 'W' => 17,
                'e' | 'E' => 18,
                'r' | 'R' => 19,
                't' | 'T' => 20,
                'y' | 'Y' => 21,
                'u' | 'U' => 22,
                'i' | 'I' => 23,
                'o' | 'O' => 24,
                'p' | 'P' => 25,
                '[' | '{' => 26,
                ']' | '}' => 27,

                // Home row: ASDFGHJKL
                'a' | 'A' => 30,
                's' | 'S' => 31,
                'd' | 'D' => 32,
                'f' | 'F' => 33,
                'g' | 'G' => 34,
                'h' | 'H' => 35,
                'j' | 'J' => 36,
                'k' | 'K' => 37,
                'l' | 'L' => 38,
                ';' | ':' => 39,
                '\'' | '"' => 40,
                '`' | '~' => 41,
                '\\' | '|' => 43,

                // Bottom row: ZXCVBNM
                'z' | 'Z' => 44,
                'x' | 'X' => 45,
                'c' | 'C' => 46,
                'v' | 'V' => 47,
                'b' | 'B' => 48,
                'n' | 'N' => 49,
                'm' | 'M' => 50,
                ',' | '<' => 51,
                '.' | '>' => 52,
                '/' | '?' => 53,

                _ => 0,
            }
        }
        _ => 0,
    };

    // XKB keycode = evdev keycode + 8
    Keycode::new(evdev_code + 8)
}
