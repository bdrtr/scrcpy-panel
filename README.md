# scrcpyrust 📱➡️🖥️

A **Rust** implementation of the [scrcpy](https://github.com/Genymobile/scrcpy) client — mirror and control Android devices from your computer.

## What Is This?

This is a rewrite of scrcpy's C client in idiomatic Rust. It communicates with the **same** Java server (`scrcpy-server.jar`) that runs on the Android device, using the same wire protocol. The server is unchanged — only the client is rewritten.

## Features (Phase 1)

- ✅ Video mirroring (H.264/H.265/AV1)
- ✅ Mouse control (click, drag, scroll)
- ✅ Keyboard shortcuts (Alt+F fullscreen, Alt+H home, Alt+B back, etc.)
- ✅ Right-click → BACK, Middle-click → HOME
- ✅ Device auto-detection
- ✅ ADB reverse/forward tunnel
- ✅ FPS counter (Alt+I)

## Requirements

- **Rust** 1.70+ (with cargo)
- **FFmpeg** development libraries (via MSYS2 on Windows)
- **ADB** (Android Debug Bridge)
- **USB Debugging** enabled on your Android device

## Quick Start

```bash
# Build
cargo build --release

# Copy scrcpy-server next to the binary
cp /path/to/scrcpy-server target/release/

# Run (with phone connected via USB)
cargo run --release
```

## Build on Windows (MSYS2)

```powershell
# Ensure MSYS2 FFmpeg is installed
D:\msys64\usr\bin\bash.exe -lc "pacman -S mingw-w64-x86_64-ffmpeg"

# Set environment for FFmpeg discovery
$env:FFMPEG_DIR = "D:\msys64\mingw64"
$env:PATH = "D:\msys64\mingw64\bin;$env:PATH"

# Build
cargo build --release
```

## CLI Options

```
scrcpyrust [OPTIONS]

Options:
  -s, --serial <SERIAL>        Device serial number
  -m, --max-size <SIZE>        Limit video resolution (e.g. 1024)
      --max-fps <FPS>          Maximum framerate
  -b, --video-bit-rate <RATE>  Video bit rate (default: 8000000)
      --video-codec <CODEC>    h264, h265, or av1 (default: h264)
      --no-audio               Disable audio
      --no-video               Disable video
      --no-control             Disable device control
      --fullscreen             Start in fullscreen
      --always-on-top          Keep window on top
      --borderless             Borderless window
  -r, --record <FILE>          Record to file (Phase 2)
      --window-title <TITLE>   Custom window title
  -S, --turn-screen-off        Turn off device screen
  -v, --version                Show version
```

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Alt+F` | Toggle fullscreen |
| `Alt+H` | Home button |
| `Alt+B` | Back button |
| `Alt+S` | App switcher |
| `Alt+P` | Power button |
| `Alt+M` | Menu button |
| `Alt+↑/↓` | Volume up/down |
| `Alt+N` | Notification panel |
| `Alt+Shift+N` | Collapse panels |
| `Alt+R` | Rotate device |
| `Alt+O` | Screen off |
| `Alt+Shift+O` | Screen on |
| `Alt+I` | Toggle FPS counter |
| Right-click | Back |
| Middle-click | Home |

## Architecture

```
src/
├── main.rs          # Orchestrator — wires everything together
├── options.rs       # CLI parsing (clap)
├── adb/             # ADB command execution & tunneling
├── server/          # Server push, params, socket connections
├── media/           # Demuxer (protocol) + Decoder (FFmpeg)
├── display/         # SDL2 window & frame rendering
├── control/         # Control message serialization & queue
├── input/           # SDL event → control message translation
└── util/            # Binary helpers, networking
```

## License

Apache 2.0 (same as original scrcpy)
