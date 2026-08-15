# scrcpy-slint

A Rust scrcpy client with a [Slint](https://slint.dev) user interface — mirror and control
Android devices from a single control panel.

> **Status: early.** The mirror now renders and takes input in a Slint window — SDL2 no
> longer draws anything. What does not exist yet is the control panel from
> [`design/`](./design/): no tabs, no configuration form, no profiles. Options still come
> from the command line.

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
| Slint window render | works — 30–61 fps sustained at 720p |
| Opus audio decode and playback | works |
| MP4 recording | works — valid file, 128 frames in 7.2 s |
| `--list-encoders` | works |
| `--time-limit` | works — enforced client side |
| Ctrl-C / SIGTERM shutdown | works — pipeline unwinds, no crash |
| scrcpy 4.1 server handshake | works, no unknown-option warnings |

The mid-stream session header — the one that arrives when the device rotates or the
mirrored app resizes — is covered by unit tests (`cargo test`) but has not been seen
against a real device: the test phone's launcher is rotation locked. It shares its parsing
with the opening session header, which every run exercises.

## Known issues

- **The server version is pinned exactly.** The server refuses to start unless the client
  announces its own version, so `SCRCPY_SERVER_VERSION` in `src/main.rs` must match the
  `scrcpy-server` you run. It is currently `4.1`, and 3.x servers are not merely rejected —
  their framing is incompatible (see below).
- `--lock-video-orientation` is gone. scrcpy removed the server option in favour of
  `--capture-orientation`, which takes degrees with an optional `@` to lock.
- The upstream README understates the code: it lists far fewer flags than `--help` actually
  accepts, and marks recording as "Phase 2" although it works.
- **UHID and AOA input are unreachable.** Those modes need hardware scancodes and Slint
  reports keys as text, so `--keyboard=uhid`, `--mouse=uhid` and OTG fall back to SDK
  injection with a warning. The HID modules are still in the tree, just unwired.
- `--always-on-top` and `--borderless` do nothing: Slint 1.17 exposes no window API for
  either.
- `--render-driver` was an SDL renderer hint and is now ignored; pick a Slint backend with
  `SLINT_BACKEND` instead.
- Each frame is copied twice on the way to the screen (swscale output → packed buffer →
  Slint pixel buffer). Fine at 1080p60, but a GPU texture path would remove both.

## Changes from upstream

- **Updated the protocol from scrcpy 3.3.4 to 4.1.** scrcpy 4.0 changed the stream
  framing: the video header no longer carries the frame size, which now arrives in a
  12-byte *session header* that can also reappear mid-stream when the size changes, and
  the config and key-frame flags moved down one bit to free the most significant bit for
  it. Upstream also sent two options the server never had — `lock_video_orientation` and
  `time_limit`; the first is dropped and the second is now enforced client side, which is
  where scrcpy has always enforced it. VP8 and VP9 codec ids added.
- **Replaced the SDL2 window and renderer with Slint.** `display/screen.rs` and
  `input/manager.rs` are gone; `ui/mirror.slint` draws the mirror and
  `input/slint_input.rs` turns pointer and key events into control messages.
- The decoder now emits packed RGB8 instead of YUV420P planes, because Slint takes pixel
  buffers where SDL took YUV textures. Its scaler is also rebuilt when the stream changes
  size or format, which upstream never did — the device rotating used to feed the old
  scaler.
- Ctrl-C and SIGTERM leave the event loop and unwind the pipeline in order. Without this
  the process exited while the decoder thread still held the FFmpeg hardware context, and
  CUDA crashed on the way out.
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
- SDL2 — no longer used for rendering, but the audio player and the clipboard helpers
  still call into it
- `adb` on `PATH`
- A `scrcpy-server` binary matching the pinned protocol version, next to the built binary

## Build and run

```bash
cargo build --release
./target/release/scrcpy-slint
```

If scrcpy 4.1 is installed, its server is found automatically at
`/usr/share/scrcpy/scrcpy-server`. Otherwise fetch it:

```bash
curl -L -o target/release/scrcpy-server \
  https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-server-v4.1
```

## Roadmap

1. ~~Replace SDL2 with a Slint window and an embedded mirror view~~ — done
2. Build the control panel from [`design/`](./design/): device list, the eight-section
   configuration form, session controls, profiles, log and shortcut tabs
3. Drive the client from that panel instead of CLI flags
4. ~~Update the protocol from scrcpy 3.3.4 to 4.x~~ — done, pinned to 4.1
5. Get input parity back: UHID and AOA keyboards, mice and gamepads
6. Drop SDL2 entirely — audio to `cpal`, clipboard to a native crate
7. GPU frame path (Slint's `unstable-wgpu-29` texture import) to remove the per-frame copies
8. Fill in what upstream left out: virtual display (`--new-display`), OTG, the rest of camera

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
ui/mirror.slint      # the mirror window: layout, rotation, input forwarding
src/
├── main.rs          # orchestrator, pipeline threads, Slint event loop
├── options.rs       # CLI parsing (clap) — to be replaced by the panel
├── ui/              # Slint bindings: orientation, frame → image
├── adb/             # ADB commands, tunnelling, sync
├── server/          # server push, parameters, socket connections
├── media/           # demuxer, decoder, recorder, delay buffer
├── display/         # FPS counter, V4L2 sink
├── control/         # control message serialisation
├── input/           # Slint events → control messages; UHID/AOA (unwired)
├── audio/           # playback and regulation
└── util/            # binary and network helpers
```

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
