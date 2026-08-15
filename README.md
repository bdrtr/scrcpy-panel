# scrcpy-slint

A Rust scrcpy client with a [Slint](https://slint.dev) user interface — mirror and control
Android devices from a single control panel.

> **Status: early.** The Rust client inherited from upstream works (see
> [Verified](#verified) below). The Slint interface is not built yet — the client still
> renders through SDL2. Replacing that is the point of this fork.

## What this is

[scrcpy](https://github.com/Genymobile/scrcpy) is split in two: a **server** written in Java
that runs on the Android device, and a **client** written in C that runs on your computer.
The server uses Android framework APIs (`MediaCodec`, `SurfaceControl`, `AudioRecord`) and
cannot be rewritten in Rust — so it stays exactly as it is, unmodified.

This project rewrites the **client** in Rust and puts a Slint control panel in front of it,
instead of scrcpy's SDL window plus command-line flags.

## Provenance

This is a fork of [naaceer-del/ScrcpyRUST](https://github.com/naaceer-del/ScrcpyRUST)
(Apache-2.0), which is itself a Rust reimplementation of the scrcpy client. Upstream is a
single-commit repository that has not been touched since it was published, and it ships no
`LICENSE` file — it declares Apache-2.0 in its README and `Cargo.toml` only. This fork adds
the license text and a `NOTICE` recording attribution and changes. See [NOTICE](./NOTICE).

Upstream's own design notes are preserved under [`docs/upstream/`](./docs/upstream/). Treat
their claims with care: the upstream commit message says "100% feature parity", which
6,500 lines of Rust against scrcpy's ~20,000 lines of C does not support.

## Verified

Tested against a Xiaomi Redmi 2209116AG (Android 13) over USB:

| Stage | Result |
| --- | --- |
| `adb push` + reverse tunnel | works |
| Server handshake, device metadata | works |
| H.264 demux | works |
| Hardware decode | works (CUDA negotiated automatically) |
| SDL2 / OpenGL render | works |
| MP4 recording | works — valid file, 128 frames in 7.2 s |
| `--list-encoders` | works |

## Known issues

- **Server version is pinned to scrcpy 3.3.4.** The version string is hardcoded in
  `src/server/params.rs`. Running against a newer server fails the handshake with
  `The server version (4.1) does not match the client (3.3.4)`. Updating to 4.x is on the
  roadmap.
- `--time-limit` is sent to the server under the wrong option name; the server logs
  `Unknown server option: time_limit` and the limit is silently ignored.
- The upstream README understates the code: it lists far fewer flags than `--help` actually
  accepts, and marks recording as "Phase 2" although it works.

## Changes from upstream

- `ffmpeg-next` / `ffmpeg-sys-next` bumped from 8 to 9 — version 8 does not compile against
  FFmpeg 9 (`libavcodec 63`)
- added the missing `libc` dependency required by `src/display/v4l2_sink.rs`
- added `LICENSE` and `NOTICE`
- removed development artifacts committed upstream (build logs, benchmark output, a
  recorded `.mkv`, agent workflow files)
- renamed the crate to `scrcpy-slint`

## Requirements

- Rust 1.70+
- FFmpeg 9 development libraries (`libavcodec`, `libavformat`, `libavutil`, `libswscale`)
- SDL2 (until the Slint interface replaces it)
- `adb` on `PATH`
- A `scrcpy-server` binary matching the pinned protocol version, next to the built binary

## Build and run

```bash
cargo build --release

# fetch the matching server (3.3.4 for now)
curl -L -o target/release/scrcpy-server \
  https://github.com/Genymobile/scrcpy/releases/download/v3.3.4/scrcpy-server-v3.3.4

./target/release/scrcpy-slint
```

## Roadmap

1. Replace SDL2 (`src/display/screen.rs`) with a Slint window and an embedded mirror view
2. Drive the client from the Slint panel instead of CLI flags
3. Update the protocol from scrcpy 3.3.4 to 4.x
4. Fill in what upstream left out: virtual display (`--new-display`), OTG, the rest of camera

The interface being built is in [`design/`](./design/) — a control panel with device
management, an eight-section configuration form covering ~85 scrcpy flags, session control,
profiles, logs and shortcuts.

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Alt+F` | Toggle fullscreen |
| `Alt+H` | Home |
| `Alt+B` | Back |
| `Alt+S` | App switcher |
| `Alt+P` | Power |
| `Alt+M` | Menu |
| `Alt+↑/↓` | Volume up/down |
| `Alt+N` | Notification panel |
| `Alt+Shift+N` | Collapse panels |
| `Alt+R` | Rotate device |
| `Alt+O` / `Alt+Shift+O` | Screen off / on |
| `Alt+I` | Toggle FPS counter |
| Right-click | Back |
| Middle-click | Home |

## Layout

```
src/
├── main.rs          # orchestrator
├── options.rs       # CLI parsing (clap) — to be replaced by the Slint panel
├── adb/             # ADB commands, tunnelling, sync
├── server/          # server push, parameters, socket connections
├── media/           # demuxer, decoder, recorder, delay buffer
├── display/         # SDL2 window and rendering — to be replaced by Slint
├── control/         # control message serialisation
├── input/           # input events → control messages, UHID/AOA
├── audio/           # playback and regulation
└── util/            # binary and network helpers
```

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
