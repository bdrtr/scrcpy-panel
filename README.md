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
| Hardware decoding | works, and is off by default because it is slower here — VAAPI opens on this machine's second render node, which the fixed per-platform list and the node search are for. In a live session on the Redmi, screen busy, it cost 6.49 ms a frame against the software path's 5.40, and 5.43 of that was the trip back from the GPU alone. Its picture is the software decoder's byte for byte — mean 0.0000 of 255, worst single byte 0, over 568 frames |
| swscale writing past the window's buffer | fixed — it writes 24 bytes past the last row at 1080x2400, 21 at 1081x2400 and nothing at all at 1080x2399, so which of three writes the client uses is measured on the first frame of each size rather than assumed. `cargo test` checks the one chosen against a whole-picture conversion at six sizes, byte for byte |
| No copies per frame instead of two | measured at 1080x2400 — 0.70 ms of conversion plus 1.10 of copying became 1.17 ms of conversion and nothing else; the picture still refreshes, two screenshots two seconds apart differing by thousands of RMSE against a still-mirror floor of about 100 |
| `--otg` | works — the device is found on the bus with no adb at all, a keyboard and a mouse are registered over USB, and the pointer's motion comes out of the phone's kernel as `REL_X`/`REL_Y` while the window has it |
| `--keyboard=aoa` | works — the AOA keyboard registers over USB during a session and is given back at the end. `cargo test -- --ignored aoa`, with `AOA_SERIAL` set, registers one and types an "a": `KEY_A DOWN`/`UP` came out of the phone's kernel with no adb, no server and no control socket in the way |
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
  twice is that `SlintInput` sends nothing to the device while UHID is attached.
- **`--keyboard=aoa` and `--mouse=aoa` take the other road: the cable.** AOA is four USB
  control transfers to the phone's own endpoint zero — register a HID device, hand it a
  report descriptor, send events, unregister — and the reports are the same bytes UHID
  sends over the socket. The device is never switched into accessory mode, which is the
  part that matters: AOA HID works while the phone stays an ordinary USB device, so adb
  keeps its own connection over the same cable and the input works on the lock screen,
  where injection does not. Asking for it without a cable — a device reached over TCP/IP —
  falls back to UHID with a line saying so, since UHID reaches the same place.
- The first AOA keypress of a session used to be refused with a USB stall, every time. The
  device builds its HID device from the descriptor *after* answering the request that
  carried it, and an event that arrives while it is still building is not a valid event;
  registration now leaves it two tenths of a second, which is more than the tenth it took
  to be ready on the phone this was found on.
- **`--otg` is the whole of the input path with nothing else attached.** No adb, no server,
  no video: the window exists only to be typed into, and says so instead of showing a
  picture. Without adb there is no device list to ask, so the USB bus is asked directly —
  anything that answers an accessory-protocol query with 2 or more is an Android device
  willing to take HID, and if exactly one does, that is the one. `--keyboard` and `--mouse`
  are forced to `aoa` there, since there is no socket for UHID to use.
- **`--gamepad=uhid` works, and is the one thing here with no gamepad to prove it on.**
  The report side was ported with the rest of scrcpy's HID code; what it lacked was a
  source, and neither Slint nor winit reads gamepads. gilrs does, on every desktop this
  runs on, so it is the one dependency taken for input. What it reports is not what the
  report wants — `hid_gamepad.rs` speaks SDL's button and axis numbering, gilrs speaks its
  own — so `input/gamepads.rs` is mostly that translation, and the translation is what the
  tests cover: the button order, the analog triggers being axes rather than buttons, and
  the sticks' vertical axis, which gilrs points up and SDL points down. gilrs is a queue to
  drain rather than something to wait on, so it is read every 8 ms on a timer, which is
  what a wired pad reports at. All of that has been run with no gamepad connected — gilrs
  starts, enumerates none, and the session is unaffected — and none of it with one.
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
- Of the 74 flags the form can produce, 73 reach the device. The one that does not is
  `--otg`, and not for want of an implementation: OTG is a session with no session in it —
  no adb, no server, no picture — and the panel's model is a mirror in a tab. The command
  line has it; the panel names it as dropped rather than pretending. An earlier version of
  this line claimed all of them, which was a miscount: the check that produced the number
  was reading the list of deliberately unimplemented flags in a test as though it were the
  supported list.
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
- The build has no warnings, which is a choice rather than a coincidence: the ones that had
  collected — 84 of them — were hiding the next real one. Most were dead helpers left over
  from the port and are gone; the gamepad and AOA ports stay, behind a module-level allow
  that says what they are waiting for. Two turned out to be worth more than a deletion: the
  audio decoder was carrying two buffers nothing wrote to, and the sample rate and channel
  count it reports were read by nobody — the session checks them against the 48 kHz stereo
  the player is built for now, and says so if a device ever disagrees.
- **A frame is not copied at all on its way to the screen now.** It used to be copied
  twice: once to take swscale's row padding off, and once more to get the packed bytes into
  a buffer Slint owns. `DecodedFrame` holds the Slint buffer itself, and swscale is pointed
  straight at it with the packed stride the window wants — `sws_scale` takes a destination
  and a stride, which `Context::run` hides behind an AVFrame of its own. Handing the frame
  over is then a refcount. What makes that safe is that the frame on screen no longer goes
  back to the decoder's pool until another has replaced it: writing into the buffer the
  window is reading from would have copied it anyway, which is the copy being removed.
- Measured on the Redmi at 1080x2400, release build, screen busy: the conversion was 0.70 ms
  a frame and the unpadding copy 1.10, so 1.80 together; it is 1.17 ms now, all of it
  conversion. The conversion itself got dearer — swscale's fast path likes an aligned
  destination and a packed RGB row of 3240 bytes is not one. Rounding the buffer up to 64
  pixels a row brings it back to 0.90 ms, but the padding would then have to be clipped in
  the UI and skipped row by row on the way to `--v4l2-sink`, and 0.27 ms a frame is not
  worth two places where a stride can be got wrong.
- **Hardware decoding could not work on this platform, and now can.** The list of hardware
  types to try was D3D11VA, DXVA2, CUDA — two Windows APIs and one that needs an NVIDIA
  card — so on a Linux desktop with any other GPU nothing could ever match, and the CUDA
  probe printed "no CUDA-capable device is detected" at error level on every launch, which
  reads like a fault and is not one. The list is per-platform now, VAAPI leads it here, and
  the probe is quiet. VAAPI's default device is the first DRM render node, which on a
  machine with two GPUs may be the wrong one — on this one `renderD128` refuses and
  `renderD129` answers — so the nodes are tried in turn.
- **Asking the GPU for the right layout was worth more than the GPU.** A hardware frame
  comes back in the GPU's own NV12, and swscale has a hand-written path from YUV420P to
  packed RGB and none from NV12: at 1080x2400, into this client's buffer, that conversion
  costs 5.07 ms a frame against 0.59 — eight and a half times over, and more than
  everything else in the path put together. This GPU will hand back sixteen formats and YUV420P is one of
  them, so the transfer asks for that one now. A driver that refuses is asked once, told so
  in the log, and never asked again. `REC=<a recording> cargo test --release -- --ignored
  --nocapture layout` is that measurement.
- That also settles an older claim in this file. The hardware and software pictures were
  said to differ by a mean of 0.81 of 255 — "two roundings of the same arithmetic". It was
  not the decoders: it was the two swscale paths, and 0.81 is what the NV12 and YUV420P
  conversions of *the same frame* differ by. Converting both from YUV420P, the two decoders
  agree bit for bit over 568 frames — mean 0.0000, worst single byte 0.
- Whether hardware is *faster* is now measured, and on this machine it is not, so
  `--hwaccel` defaults to `off`. The frames have to come back to system memory for swscale
  either way, and that trip is the whole story. In a live session on the Redmi, screen busy,
  release build: 6.49 ms a frame with the GPU against 5.40 for the whole software path, and
  5.43 of the hardware figure was the readback alone — as much as the software path costs
  end to end, though 5.43 against 5.40 is close enough to be one run's luck. Through a
  recording off the same device it is not close: 568 frames at 1080x2400, 4.90 against 2.96,
  and 0.59 of each is a colour conversion both paths pay, so decoding on the GPU and
  fetching the result is 4.31 ms a frame where the CPU decodes in 2.37. `--hwaccel auto` is
  still there for a machine where that comes out the other way, which is the only reason the
  flag exists.
- **swscale was writing past the end of the window's buffer, and the client now measures
  whether it will.** Pointing swscale straight at Slint's buffer is what saved the copy a
  frame above, but its hand-written YUV420P path converts a block of pixels at a time and
  writes the whole last block: it fills the row out to a multiple of sixteen pixels, so
  1080 wide spills 24 bytes past the end of every row, 1081 spills 21, and 64 spills none.
  Every row's spill but the last lands in the row below, which is written next and covers
  it; the last row's lands past the end of a buffer that is exactly `width * height * 3`
  bytes, in memory the client does not own. That is not a theoretical complaint: made to
  write straight in at 1080x2400 anyway, this client does not survive the first frame —
  glibc fails an assertion in `sysmalloc` and the process takes SIGABRT. So the last rows are converted into a buffer
  with room to spare and copied in — one or two of them: two when the height is even,
  because a 4:2:0 chroma plane cannot begin on an odd row, which is 6480 bytes of memcpy a
  frame at 1080 wide.
- What cannot be assumed is that this is needed, or even that it is safe. One row shorter,
  at 1080x2399, swscale takes a different converter for the odd height: it writes nothing
  past the picture at all, and it interpolates chroma down the frame, so cutting the tail
  off there would have left the two rows above the cut wrong by up to 255 of 255. Rather
  than reason about which converter swscale picked, the client converts the first frame of
  each size into a buffer with room to spare, reads the room back to see how far past it
  wrote, and — if it wrote anything — checks the split against that same whole picture
  before using it. Nothing past means write straight in; something past and a matching
  split means the split; something past and a split that does not match means the whole
  picture goes through a buffer and is copied in, which nothing this client decodes has
  needed. `cargo test` checks the write actually chosen against a whole-picture conversion
  at six sizes, odd widths and odd heights included.
- The room is filled and read back twice, with a different byte each time, and that is not
  belt and braces. What swscale writes past the picture is picture, and a picture can be any
  byte: a frame whose last rows convert to 0xAA leaves a buffer filled with 0xAA looking
  untouched, and the client would then write out of bounds for as long as that session ran.
  A byte it wrote can match only one of two fillings, so a byte both runs left alone was
  left alone. There is a test that builds exactly that frame — black but for a corner of the
  grey that converts to 0xAA — and it does slip past one filling and not past two.
- The figures above come from two harnesses and are not interchangeable. The same colour
  conversion is 0.59 ms a frame through the recording and 0.92 in this session's live run,
  against the 1.17 the bullet further up measured live in its own. Every comparison here is
  between two numbers from the same harness.
- The software decoder is single-threaded on purpose, and says so now rather than
  inheriting it from libavcodec's default. Frame threading is the fast kind for H.264 and
  holds back a frame per thread before letting the first one out: fifty milliseconds at
  four threads and sixty frames a second, added to every touch, on a window whose purpose
  is to be touched. Slice threading has no such delay and no such gain either, since the
  server's encoder writes one slice a frame. At 2.96 ms a frame — decode and colour
  conversion together — the one thread carries 338 frames a second, and the stream arrives
  at sixty.
- **The window costs more than the decoder, which this file did not know.** Everything above
  measures the decoder, and the decoder was never the expensive half. Slint takes the whole
  frame to the card every time it changes, and at 1080x2400 that is eight megabytes and
  3.87 ms — against 2.96 for decoding and converting the same frame. Handing back the same
  buffer instead costs 0.02, so it is the traffic and not the drawing. It scales with the
  bytes and nothing else: 7.8 MB costs 3.87 ms, 3.9 costs 2.14, 0.5 costs 0.42. That is
  what makes the shader worth writing — YUV420P planes are 3.9 MB where the RGB is 7.8, so
  the upload halves and the conversion goes altogether. `cargo run --release --example
  frame_cost` measures it, six buffers deep because that is the session's frame pool, and
  `frame_cost 1080x1200` is the half-size figure: the same byte count as a 1080x2400 frame
  in YUV420P, which is the closest this can get to the answer without writing the thing.
  What it does not measure: three plane uploads have three lots of per-call overhead where
  this has one, and the shader costs GPU time this does not count. 2.3 ms a frame is
  therefore the ceiling on the prize, not a promise.
- The flip is the one thing that still costs a pass, and cannot stop: a mirror cannot be
  done in the buffer the window is reading from, so `--display-orientation=flipN` builds a
  second one.

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
5. ~~Get input parity back: UHID keyboard and mouse, gamepads, AOA and OTG~~ — done, bar a
   gamepad to try the gamepads on
6. ~~Drop SDL2 entirely~~ — done; audio is `cpal`, clipboard is `arboard`
7. GPU frame path: ~~the CPU copies~~ — gone, swscale writes into Slint's own buffer. What
   is left is bigger than this list used to say, and most of it is not the conversion.
   Taking eight megabytes of packed RGB to the card costs 3.87 ms a frame at 1080x2400
   against the conversion's 0.59, and half those bytes cost 2.14 — so a shader fed the YUV
   planes (Slint's `unstable-wgpu-29` texture import) is worth about 2.3 ms a frame, not
   the 0.59 the conversion is. `cargo run --release --example frame_cost` is the
   measurement. Still to find out: what three plane uploads cost against one, and what the
   shader costs on the GPU side, neither of which this measures
8. ~~Fill in what upstream left out: virtual display (`--new-display`), the rest of camera,
   OTG~~ — done

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
├── input/           # Slint events → control messages; UHID keyboard and mouse
├── audio/           # playback and regulation
└── util/            # the terminal's title
```

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
