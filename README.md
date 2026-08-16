# scrcpy-slint

A Rust scrcpy client with a [Slint](https://slint.dev) user interface — mirror and control
Android devices from a single control panel.

> **Status: usable.** `--panel` opens the control panel from [`design/`](./design/) — seven
> tabs, the eight-section configuration form, a live command preview — and the mirror runs
> inside it. `scrcpy-slint` with no flags still mirrors straight into a window of its own.

## What this is

[scrcpy](https://github.com/Genymobile/scrcpy) is split in two: a **server** written in Java
that runs on the Android device, and a **client** written in C that runs on your computer.
The server uses Android framework APIs (`MediaCodec`, `SurfaceControl`, `AudioRecord`) and
cannot be rewritten in Rust — so it stays exactly as it is, unmodified.

This project rewrites the **client** in Rust and puts a Slint control panel in front of it,
instead of scrcpy's SDL window plus command-line flags.

## Provenance

This project started as a fork of
[naaceer-del/ScrcpyRUST](https://github.com/naaceer-del/ScrcpyRUST) (Apache-2.0), a Rust
reimplementation of the scrcpy client. It now lives on its own — the Slint interface, the
4.1 protocol and the control panel are new — but the client skeleton came from there. The
fork it grew out of is kept at
[bdrtr/scrcpy-slint](https://github.com/bdrtr/scrcpy-slint) as the provenance trail. Upstream is a
single-commit repository that has not been touched since it was published, and it ships no
`LICENSE` file — it declares Apache-2.0 in its README and `Cargo.toml` only. This fork adds
the license text and a `NOTICE` recording attribution and changes. See [NOTICE](./NOTICE).

Upstream's own design notes are preserved under [`docs/upstream/`](./docs/upstream/). Treat
their claims with care: the upstream commit message says "100% feature parity", which
6,500 lines of Rust against scrcpy's ~20,000 lines of C does not support.

## Verified

Tested against a Xiaomi Redmi 2209116AG (Android 13) over USB:

| What | Result |
| --- | --- |
| `adb push` + reverse tunnel | works |
| Server handshake, device metadata | works |
| H.264 demux, hardware decode | works — CUDA negotiated automatically |
| Slint window render | works — 25–61 fps at 720p |
| Mirror embedded in the panel | works — live at 1080x2400, correct letterbox |
| Control panel | works — adb detection, device list, command preview, profiles |
| Audio through cpal | works — 48 kHz stereo Opus, no SDL |
| Session metrics | works — resolution, frame rate, codec, rotation, elapsed |
| `--record` | works — 287 frames over 9.72 s from a 30 fps camera source |
| `--new-display` virtual display | works — `New display: 1600x900/240 (id=2)` |
| `--video-source=camera` | works — 1600x1200 at a steady 30 fps |
| `--crop` | works — 1080x2400 cropped to 1080x1200 |
| `--record-format` | works — `mkv` overrides a `.mp4` filename |
| `--new-display` with no value | works — device picks the size |
| Contradictory `--no-audio --require-audio` | rejected with a message |
| `--no-video-playback` | works — 170 frames recorded with nothing drawn |
| `--mouse-bind` | works — parsed, with 6 tests; malformed input warns and keeps the default |
| Ayarlar: adb path, adb port, record dir, screenshot dir | consulted at runtime |
| Ayarlar: autostart profile, version check, log to disk | work |
| Recording started and stopped mid-session | works — 568 video + 552 Opus frames over 11 s |
| Recording with audio | works — this had never worked; every test had used --no-audio |
| Several devices selected and started together | implemented, needs a second device to try |
| `--max-size`, `--max-fps`, `--time-limit` | work |
| `--list-encoders/-displays/-cameras/-apps` | work |
| `--start-app` | works — server resolves and launches the package |
| `--verbosity` | works — server logs at the requested level |
| `--select-usb` / `--select-tcpip` | work, naming what is connected when nothing matches |
| `--new-display` with `--no-vd-*` | accepted by the server |
| scrcpy 4.1 server handshake | works, no unknown-option warnings |
| Ctrl-C / SIGTERM shutdown | works — pipeline unwinds, no crash |

Tested against a Samsung Galaxy Tab S9 FE (SM-X510, Android 16) over wireless adb:

| What | Result |
| --- | --- |
| `--no-window` | works — 374 frames in 9.05 s through a four-frame channel, recording throughout |
| `--flex-display` | works — the display followed the window three times in one session, 948x492 → 472x492 → 948x492 as a second window came and went |
| `--flex-display` with `--display-orientation 90` | works — the same window asked for 492x948 |
| `--render-fit` | works — the backdrop measured in a screenshot: letterbox 142322 px, stretched 0, unscaled 71618 |
| `--background-color` | works — that backdrop is the colour asked for |
| MOD+t / MOD+Shift+t | works — the server logs the torch going on and off |
| MOD+Up / MOD+Down on a camera | works — the server reports the zoom at 1.0625, then 1.1289, then back |
| MOD+z / MOD+Shift+z | works — frozen, two screenshots differ by 105 RMSE; running, by 573 |
| MOD+Shift+arrows | works — the flips compose, H then V leaving a half turn and no mirror |
| MOD+q | works — a session with `--time-limit 30` ended after 4 s |
| MOD+Shift+r | works — the device encodes again, the stream continues |
| `--pause-on-exit=if-error` | works — waits only on the run that failed |
| Terminal title | works — written into a pty and taken back at the end |
| SCAN_FILE after a push | accepted — a POWER keycode sent straight after it still reached the device, so the control channel survived the message |
| Camera sessions refuse what a camera cannot answer | works — Home, copy, paste and Power sent nothing, and the torch that followed still arrived |
| `--keyboard=uhid` | works — a new input device called "scrcpy" appears in `getevent -pl` while the session runs, and goes when it ends |
| `--mouse=uhid` | works — the device it adds has REL_X, REL_Y and the two wheels; a report of 30,30 came out of the phone's kernel as `REL_X 0x1e`, `REL_Y 0x1e`, and a click as `BTN_MOUSE DOWN`/`UP`. The pointer moving on this desk arrived the same way, 655 relative events in fourteen seconds |

`--keyboard=uhid` was verified in two halves, because nothing here can type into its own
window and be believed. (The mouse needed no such split: the pointer was already moving.) The half above the client: keys pressed in the panel window arrived
at the winit handler as positions — `Code(KeyA)`, `Code(KeyS)`, `Code(KeyD)`, pressed and
released, not synthetic. The half below it, on the Redmi over USB: HID reports for usages
0x08, 0x09 and 0x0A came out of the device's own kernel as `KEY_E`, `KEY_F` and `KEY_G`,
read from `getevent -lt` on the `scrcpy` input device. The join between the two halves —
position to usage id — is the table in `input/winit_keys.rs`, which has tests of its own.

Recording a mostly static screen produces a file shorter than the session, which
is correct rather than a bug: scrcpy's encoder only emits a frame when the
surface changes, so the last timestamp is the last thing that moved.

The mid-stream session header — the one that arrives when the stream changes size — has
now been seen against a real device, three times in one session: `--flex-display` resizes
the device's display while it runs, and each resize came back as a session header with the
client-resized flag set. This paragraph used to say it had only ever been unit-tested.

## Known issues

- **The server version is pinned exactly.** The server refuses to start unless the client
  announces its own version, so `SCRCPY_SERVER_VERSION` in `src/main.rs` must match the
  `scrcpy-server` you run. It is currently `4.1`, and 3.x servers are not merely rejected —
  their framing is incompatible (see below).
- `--lock-video-orientation` is gone. scrcpy removed the server option in favour of
  `--capture-orientation`, which takes degrees with an optional `@` to lock.
- The upstream README understates the code: it lists far fewer flags than `--help` actually
  accepts, and marks recording as "Phase 2" although it works.
- **`--keyboard=uhid` and `--mouse=uhid` work now; OTG does not.** This entry used to say
  the position of a key was out of reach, because Slint reports keys as text and deriving a
  position from a character would apply the layout twice — which is the one thing UHID
  exists to avoid. The position was there all along, one layer down: Slint runs on winit,
  and `BackendSelector::with_winit_custom_application_handler` hands the raw winit events
  to a handler of one's own — `physical_key` on the keyboard side, `DeviceEvent::MouseMotion`
  on the other. `src/input/uhid.rs` turns those into the HID reports `hid_keyboard.rs` and
  `hid_mouse.rs` were already able to build, and the device applies its own layout and its
  own pointer acceleration to them. The events are passed on rather than swallowed, so
  Slint keeps the modifier state its shortcut layer reads; what stops an input arriving
  twice is that `SlintInput` sends nothing to the device while UHID is attached. OTG is
  still out: it needs USB and AOA rather than a control socket, and so does `--gamepad`,
  which additionally has no source — winit reports no gamepads.
- A UHID mouse is a relative mouse, so the pointer is captured while it runs, as it is in
  scrcpy: the window locks it where the compositor allows that and confines it to the
  window where it does not. LAlt, LSuper or RSuper give it back, and take it again. The
  rule for that is stricter here than upstream's: a capture key counts only when it is
  pressed and released with *nothing* in between, because LAlt is both a capture key and
  the usual shortcut modifier, and MOD+f should not hand over the mouse on its way to
  fullscreen. Losing the window's focus gives the pointer back as well — a grab that
  outlives the focus is how a desktop ends up with a mouse nobody can move.
- `--v4l2-sink` publishes the mirror as a webcam: `VIDIOC_S_FMT` once, then a write per
  frame. Verified end to end against a loopback device — the phone's screen read back out
  of `/dev/video9` by ffmpeg at the right size and in the right colours, with and without
  `--v4l2-buffer`. It needs the module first:
  `sudo modprobe v4l2loopback video_nr=9 card_label=scrcpy exclusive_caps=1`.
- Of the 74 flags the form can produce, 72 reach the device. The two that do not are
  `--otg` and `--gamepad`, which need a scancode source and a gamepad source the Slint
  window does not have; the panel names them as dropped rather than pretending. An earlier
  version of this line claimed all of them, which was a miscount: the check that produced
  the number was reading the list of deliberately unimplemented flags in a test as though
  it were the supported list.
- **The command line is scrcpy 4.1's, less three.** Comparing the two `--help` outputs
  leaves `--otg` and `--gamepad`, which want an input source Slint does not have, and
  `--no-window-aspect-ratio-lock`, which has nothing to turn off: SDL3 can lock a window to
  the video's aspect ratio and neither Slint nor winit 0.30 exposes anything of the kind, so
  the window is already free to be any shape and the picture letterboxes inside it. Going
  the other way, this client's `--help` has flags scrcpy's does not — `--panel`, `--start`,
  `--server-path`, `--clipboard-direction`, and the positive halves of switch pairs like
  `--power-on`.
- `--no-downsize-on-error` exists because `--downsize-on-error` cannot say no. A `bool`
  field is a switch to clap, and a switch's default is the value it has when absent, so
  `default_value = "true"` makes a flag that is true whether or not it is given. The same
  is true of `--forward-key-repeat`, `--power-on` and `--clipboard-autosync`, and each has
  a `--no-` counterpart that is the one doing the work.
- `--flex-display` resizes the device's display to follow the window, which is a control
  message (`RESIZE_DISPLAY`, id 21) rather than a server option — `flex_display=true` only
  says the display may be resized. Slint reports no resize event, so the window size is
  polled every 150 ms and sent once it stops changing; the device's answer arrives as a new
  stream size, which resizes no window here, so there is no feedback loop. A quarter turn
  of the client rotation swaps the size asked for, since the window shows the picture
  rotated. It wants a virtual display to work on: with `--new-display`, `--flex-display`
  makes it flexible. Under a tiling compositor the window size is not the user's to drag —
  COSMIC ignores `set_size` entirely — so the display follows the tile instead, and changes
  with it when another window opens beside the mirror. That also makes the two window-sizing
  shortcuts, MOD+w and MOD+g, do nothing there.
- `--render-fit` is three ways of filling the window: `letterbox` keeps the shape, which is
  what this client always did, `stretched` gives it up, and `unscaled` draws one video pixel
  per screen pixel and clips. The three are a Slint enum bound to the same rectangle the
  pointer coordinates are normalised against, so a click lands in the right place under all
  of them. Unset means `letterbox`, or `unscaled` alongside `--flex-display`, where the
  display is the window's size already.
- `--no-window` runs the session with nothing drawing it — for recording, or for publishing
  the screen to `--v4l2-sink`. The frames are still decoded and thrown away rather than not
  decoded at all: recording is fed from the demuxer, but the decoder sits behind the same
  packet channel, so a decoder nobody reads from blocks the demuxer and stops the recording
  with it. There is no Slint event loop either, so the interrupt and `--time-limit` are read
  by the drain loop rather than by timers.
- `--no-terminal-title` turns off the one thing this client writes to the terminal that is
  not a log line: `ESC ]0;<title> BEL`, the same escape scrcpy writes, with the window's
  title in it. It is written only when standard output is a terminal, and taken back when
  the session ends.
- **The server has two control handlers, and a camera session's is small.** Mirroring a
  camera it takes the torch, the two zoom steps and a video reset; anything else — a touch,
  a key, the clipboard — is a protocol error it answers with an AssertionError on its
  control thread, which ends the control channel for the rest of the session. So the client
  holds those back while mirroring a camera: the pointer and keyboard send nothing, the
  shortcuts that reach the device are refused, and the panel's clipboard button says why.
  This was found by sending a scan-file to a camera session and watching the thread die.
- A pushed file is followed by a scan request, so it turns up in the gallery rather than
  only in the filesystem. scrcpy hands the device the target directory rather than the file,
  and so does this — one request for a batch. `--push-target` says where the file goes and
  what the scan names; until now it was a flag the parser accepted and nothing read. On
  Android 11 and above adbd indexes what it pushes anyway, so on the test tablet the scan
  changed nothing observable; it is what scrcpy does, and older devices need it.
- The two-finger gestures are on Ctrl and Shift now, where scrcpy has them, rather than on
  the shortcut modifier: Ctrl+drag pinches and rotates about the centre, Shift+drag slides
  two fingers up and down, Ctrl+Shift+drag slides them left and right. It is one mechanism
  — a second finger mirrored through one axis, the other, or both — and which axes are
  mirrored is decided when the button goes down, so letting the modifier go mid-drag does
  not change the gesture under way.
- MOD+z freezes the picture without pausing the stream. The frames keep arriving, are
  decoded, and go back to the pool undrawn; stopping the stream instead would mean asking
  the device for a fresh keyframe to start again, which is what MOD+Shift+r is for.
- `--adb-port` reaches both paths to the daemon: adb's own command line through
  `ANDROID_ADB_SERVER_PORT`, and `src/adb/protocol.rs`, which reads the same variable
  rather than the 5037 it used to hardcode.
- The interface has two languages. The strings in `ui/` go through Slint's `@tr`, and the
  panel's own messages go through `tr!` in `src/i18n.rs`; both read the same
  `lang/en/LC_MESSAGES/scrcpy-slint.po`, which slint-build bundles for the first and
  build.rs turns into a sorted table for the second. Switching is a call rather than a
  restart. Turkish is the source language, so anything absent from the .po falls back to
  it — which is what codec names, example paths and the program's own name want.
- Minimize-to-tray works, with two workarounds around Slint 1.17.1: the generated
  `show()`/`hide()` on a tray-rooted component panic, because `visible` is frozen as
  constant when no binding in the file writes it; and the platform handle is built from
  the icon alone without consulting `visible`, so an icon bound to false is still shown.
  Presence is therefore controlled by creating and dropping the component, which is the
  documented lifecycle anyway.
- `--clipboard-direction` is this client's own, not a scrcpy option: scrcpy syncs the
  clipboard both ways or not at all, and the panel's mockup asks for a direction as well.
  `to-device` keeps the phone's clipboard off this machine; `to-pc` keeps this machine's
  clipboard off the phone. Both ends of the sync are on different threads, so the policy
  is process-wide rather than threaded through both.
- Drag-and-drop file transfer is not possible: Slint 1.17's `DataTransfer` exposes plain
  text and images, not dropped file paths. The transfer box takes a click instead and
  opens the desktop's own file chooser over the XDG portal, which reaches the same two
  outcomes — an APK is installed, anything else is pushed to `/sdcard/Download`.
- `--borderless` is Slint's own `no-frame` on the Window element. Nothing else works: the
  winit adapter re-applies decorations from that property every time it updates window
  properties, so a decoration set on the winit window directly, or through the window
  attributes it was created with, is overwritten moments later. Verified on COSMIC.
- `--always-on-top` has no Slint property and is a window attribute rather than something
  set afterwards, so it goes through `with_winit_window_attributes_hook`. Wayland leaves
  stacking to the compositor and ignores it; X11 honours it.
- `--render-driver` was an SDL renderer hint and is now ignored; pick a Slint backend with
  `SLINT_BACKEND` instead.
- `--disable-screensaver` went with SDL and came back over D-Bus: the session holds an
  `org.freedesktop.ScreenSaver` inhibition, which GNOME, KDE and COSMIC all answer, for as
  long as it runs. A desktop without that service logs a warning and mirrors anyway.
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
- `adb` on `PATH`
- A `scrcpy-server` binary matching the pinned protocol version, next to the built binary

## Build and run

```bash
cargo build --release

./target/release/scrcpy-slint            # mirror in a window of its own
./target/release/scrcpy-slint --panel    # the control panel
./target/release/scrcpy-slint --panel --start   # panel, already mirroring
```

If scrcpy 4.1 is installed, its server is found automatically at
`/usr/share/scrcpy/scrcpy-server`. Otherwise fetch it:

```bash
curl -L -o target/release/scrcpy-server \
  https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-server-v4.1
```

## Roadmap

1. ~~Replace SDL2 with a Slint window~~ — done
2. ~~Build the control panel from [`design/`](./design/)~~ — done
3. ~~Embed the mirror in the panel~~ — done; Ayarlar switches between embedded and a
   window of its own
4. ~~Update the protocol from scrcpy 3.3.4 to 4.x~~ — done, pinned to 4.1
5. Get input parity back: ~~UHID keyboard and mouse~~ — done, through winit's raw events;
   AOA and gamepads need USB and a gamepad source, and are what is left
6. ~~Drop SDL2 entirely~~ — done; audio is `cpal`, clipboard is `arboard`
7. GPU frame path (Slint's `unstable-wgpu-29` texture import) to remove the per-frame copies
8. Fill in what upstream left out: ~~virtual display (`--new-display`)~~ and ~~the rest of
   camera~~ — done; OTG remains, and waits on the same scancode source as 5

The interface being built is in [`design/`](./design/) — a control panel with device
management, an eight-section configuration form covering ~85 scrcpy flags, session control,
profiles, logs and shortcuts.

## Keyboard shortcuts

Alt is the modifier unless `--shortcut-mod` says otherwise; scrcpy writes it MOD.

| Shortcut | Action |
|----------|--------|
| `Alt+Q` | Quit |
| `Alt+F`, `F11` | Toggle fullscreen |
| `Alt+W`, double-click the bars | Fit the window to the picture |
| `Alt+G` | Resize to 1:1 |
| `Alt+←` / `Alt+→` | Rotate the picture |
| `Alt+Shift+←/→` | Flip the picture horizontally |
| `Alt+Shift+↑/↓` | Flip the picture vertically |
| `Alt+Z` / `Alt+Shift+Z` | Freeze the picture / let it run |
| `Alt+Shift+R` | Encode again from a fresh keyframe |
| `Alt+I` | Toggle the FPS counter |
| `Alt+H`, middle-click | Home |
| `Alt+B`, `Alt+Backspace`, right-click | Back |
| `Alt+S`, 4th-click | App switcher |
| `Alt+M` | Menu |
| `Alt+P` | Power |
| `Alt+↑/↓` | Volume up/down, or the camera zoom in camera mode |
| `Alt+O` / `Alt+Shift+O` | Device screen off / on |
| `Alt+R` | Rotate the device |
| `Alt+N` / `Alt+Shift+N`, 5th-click | Expand / collapse the notification panel |
| `Alt+C` / `Alt+X` | Copy / cut to the computer |
| `Alt+V` / `Alt+Shift+V` | Paste the computer's clipboard / type it |
| `Alt+K` | Open the keyboard settings |
| `Alt+T` / `Alt+Shift+T` | Camera torch on / off |
| Ctrl+drag | Pinch and rotate about the centre |
| Shift+drag | Slide two fingers up and down |
| Ctrl+Shift+drag | Slide two fingers left and right |

## Layout

```
ui/
├── app.slint        # the root: every window re-exported from one file
├── mirror_view.slint# the mirror itself, shared by both hosts, on a Mirror global
├── mirror.slint     # a window that is a frame around MirrorView
├── panel.slint      # the control panel's chrome
├── theme.slint      # design tokens transcribed from the mockup
├── components.slint # the component library, keyed to the mockup's CSS classes
├── state.slint      # Cfg / Settings / App globals
├── config/          # the eight configuration sections
└── tabs/            # the six non-configuration tabs
src/
├── main.rs          # entry point and the standalone mirror window
├── session.rs       # a session without a window: server, tunnel, decode threads
├── mirror_host.rs   # drives a MirrorView wherever it is mounted
├── panel/           # the control panel: command building, devices, profiles
├── options.rs       # CLI parsing (clap)
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
