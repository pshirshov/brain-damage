# termui

Run graphical Wayland applications in the terminal using the Kitty graphics protocol.

<img width="1852" height="1288" alt="image" src="https://github.com/user-attachments/assets/20790637-72b4-47e2-b545-e6a4f98ddeee" />

Yes, you see it right. It's kitty running in kitty under a Wayland compositor which renders it into kitty through kitty's graphics protocol.

Why? Because fuck you. 

It wasn't me, it was Claude.

## Overview

termui acts as a minimal Wayland compositor that:
- Captures frames from Wayland applications
- Renders them in the terminal using Kitty's graphics protocol
- Translates terminal input (keyboard/mouse) back to Wayland events

## Requirements

- A terminal supporting the Kitty graphics protocol (e.g., Kitty)
- Nix with flakes enabled (for dependencies)

## Building

```bash
nix develop
cargo build --release
```

## Usage

```bash
./target/release/termui <command> [args...]
```

### Examples

```bash
# Run foot terminal
termui foot

# Run a GTK4 application
termui gtk4-demo
```

### Controls

- `Ctrl+Q` or `Ctrl+C` - Exit termui

## How it works

1. termui creates a Wayland socket and spawns the target application
2. The application renders to shared memory buffers (wl_shm)
3. termui captures frames on each surface commit
4. Frames are encoded and sent to the terminal via Kitty graphics protocol
5. Terminal input events are translated to Wayland pointer/keyboard events

## Limitations

- Only supports wl_shm (software rendering) - no GPU acceleration
- Input latency depends on terminal and frame rate
- Some applications may not work correctly

## License

MIT
