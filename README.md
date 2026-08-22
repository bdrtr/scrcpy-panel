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

### What the server actually does

The server is not part of this repository and is not built here: it is
`/usr/share/scrcpy/scrcpy-server` from the distribution's own scrcpy package, unmodified.
That makes several of this client's decisions assumptions about somebody else's code, so
they were checked against the v4.1 server's source rather than left as beliefs:

| The client assumes | The server, in v4.1 |
|---|---|
| Scroll goes on the wire as signed 16-bit fixed point, one notch being `i16::MAX` | `Binary.i16FixedPointToFloat` is `value / 2^15`, with `0x7fff` special-cased to exactly 1.0 — so a bare notch count of 1 arrives as 0.00003, which is what it used to send |
| Injected text is capped at 300 bytes | `ControlMessageReader.INJECT_TEXT_MAX_LENGTH = 300` |
| A string cut to fit must be cut on a character | `DeviceMessageWriter` uses `StringUtils.getUtf8TruncationIndex` for exactly that, in the other direction |
| A device message is at most 256 KiB | `DeviceMessageWriter.MESSAGE_MAX_SIZE = 1 << 18` |
| An audio codec id of 0 means the device could not capture audio | `Streamer.writeDisableStream`: "code 0: it explicitly disables the stream (because it could not capture audio), scrcpy should continue mirroring video only" |

All 23 control message types and all 3 device message types match by name and by id, so
the client covers the 4.1 protocol with nothing missing and nothing invented. v4.1 is also
the newest release upstream has, so there is no version to catch up to.

Everything checked came out in the server's favour: where the two disagreed it was this
client that was wrong, and each of those is now fixed. There is nothing to send upstream.

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
| `--v4l2-sink`, and the whole path through it | works — with the phone attached and the mirror publishing to a `v4l2loopback` at 1080x2400, a frame read back out with `ffmpeg -f v4l2` matches the phone's own `screencap` to an RMSE of 558 of 65535, 0.85%, and to 576 and 671 on two later runs. The same comparison with the loopback frame's red and blue exchanged is 5306, ten times worse, so the channel order is right rather than lucky. That is the device's screen, encoded on the device, decoded here into RGBA, packed back down to RGB24 and published, all held against what the phone says it was showing |
| Hardware decoding, live rather than against a recording | works — with `--hwaccel auto` the published frame reads 612 of 65535 against the phone's own `screencap` and 5284 with its red and blue exchanged, so VAAPI, the YUV420P the transfer asks the driver for and the conversion after it all put the right picture out in a real session. Until now that was only known from a recording fed through the decoder offline |
| The two decoders agree in a live session too | works — both run against the same still screen and held to the same `screencap`: software 671, hardware 612, and the two published frames 277 against **each other**. The screen itself moved 295 between the two reference shots, so the decoders differ by less than the thing being photographed did — indistinguishable at this resolution, which is what `whether_the_two_decoders_agree` says exactly over a recording |
| The write is chosen again when the stream changes size | works, on the device — with the display changed mid session from 1080x2400 to 720x1600 and back, the log reads `Tail`, then `Direct`, then `Tail` again. 720 is a multiple of sixteen so that converter writes nothing past the row and the direct write is safe there; 1080 is not, so it is not. Nothing was left over from the size before, which is the failure this would have shown |
| A session outliving what it talks to | works — killing the server on the device mid session, and separately killing the local adb daemon, both end in `End of video stream`, an orderly shutdown and exit code 0, within milliseconds. Which matters here more than most places: this machine's phone drops off the bus on its own |
| The wheel | works now, and did not before — the scroll field is 16-bit fixed point and a notch count was being written into it whole, so the device was told to scroll 1/32768 of a step. `cargo test` holds one notch to `i16::MAX` on the wire, and putting the old arithmetic back makes it fail |
| A fourth byte a pixel | works, and is worth 3 ms a frame — a live session on the Redmi with the screen scrolling spent 4.04 ms a draw over 1500 draws before the change and 0.98 over 1520 after, the same probe on the same window. The picture is the same one: RGBA against packed RGB differs by 0.000 of 255 on the two renderers whose snapshots can be trusted, and the hardware decoder, the mirror and `--display-orientation=flipN` all ran clean |
| swscale writing past the window's buffer | fixed — it fills the row out to a multiple of sixteen pixels, which into RGBA is 32 bytes past the last row at 1080x2400 and 28 at 1081x2400, and nothing at all at 1080x2399 where a different converter runs. Which of three writes the client uses is measured on the first frame of each size rather than assumed, and `cargo test` checks the one chosen against a whole-picture conversion at six sizes, byte for byte |
| No copies per frame instead of two | measured at 1080x2400 — 0.70 ms of conversion plus 1.10 of copying became 1.17 ms of conversion and nothing else; the picture still refreshes, two screenshots two seconds apart differing by thousands of RMSE against a still-mirror floor of about 100 |
| `--otg` | works — the device is found on the bus with no adb at all, a keyboard and a mouse are registered over USB, and the pointer's motion comes out of the phone's kernel as `REL_X`/`REL_Y` while the window has it. Its keys and its mouse buttons did **not**, until a review looked: they travel through the window-event path, which turned everything away unless there was a control socket to send it down — and OTG is the one mode that has none. Pointer motion arrives as a *device* event and never passed that gate, which is exactly why this row could be written and the bug still be there |
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

Every adb call the client makes now goes through one module, `src/adb/device.rs`, and that
move is verified in two halves as well. Reading adb's output is tested against the bytes it
writes — a device list with a mix of `device`, `offline` and `unauthorized` rows, an empty
one, an install that reports `Failure` on stdout and exits zero anyway. Running the commands
is tested against the real adb with nothing plugged in: `cargo test --release -- --ignored
what_adb_says` asks for the list and gets an empty one rather than an error or a phantom
row, reads the version banner back, connects to a closed port and gets an error rather
than a line of news, and queries a device that is not there without hanging. That last pair
found something: `adb connect` exits 0 whether it worked or not — a closed port, an
unroutable address and a name that will not resolve all print a refusal and return success
— so the panel had been showing a failed connect as an ordinary info line and refreshing
the device list underneath it. It reads adb's words now, and the three refusals adb 1.0.41
actually prints are in the test. What that cannot cover is the other
half — no adb operation has been run through the new module against a phone yet, so
`tcpip`, `pair`, `install`, `push` and `screencap` are, for now, only as good as their
arguments look. The push, at least, is no longer among the untried: it has a fake daemon
of its own — sixty lines of `TcpListener` that speaks enough of adb's protocol to take
one — which holds the framing to account rather than skipping to the end. The transport,
the switch to sync mode, the path with its mode, 100 KB arriving as two chunks because a
chunk is 64 KB, the modification time on DONE, the file coming back byte for byte, and a
daemon answering FAIL having its words carried out to the caller.

### The files no review had looked at

Every hunt so far had gone through the same places. This one went through the ones that
had never been read: the pixel maths, the key tables, the pad translation, the audio
decoder, the adb framing, the control queue, the error cards. Seventeen things came back,
and none of them needed a phone to find. What each was held to:

- **Five of the fourteen Android metastate flags were somebody else's flags.** The meta
  row had slipped one slot, so `META_ON` was `META_META_ON | META_META_LEFT_ON` and the
  device was told the *left* Super key was down whichever one had been pressed; the two
  lock flags were `META_CAP_LOCKED` and `META_ALT_LOCKED`, three hex digits away from the
  ones meant. Checked against `android/view/KeyEvent.java` in android-36's
  `android-stubs-src.jar`, and all fourteen are now pinned by a test.
- **`--audio-codec=aac` had never decoded anything, and `--audio-codec=raw` never
  opened.** The sample rate and channel layout were set only for Opus, and they are the
  only description of the stream a decoder here gets — the config packet is thrown away
  rather than handed over as extradata. Measured against this machine's libavcodec with
  twenty AAC access units out of an mp4: 0 of 20 accepted with the rate unset, 20 of 20
  accepted and 19 frames out with it set. PCM does not even open — `avcodec_open2`
  returns EINVAL for a rate of zero — so raw lost the recording's audio track as well.
  Proved against libavcodec rather than against the server's own stream; the device end
  of that is still to do.
- **The panel showed the wrong card for four of its own failures.** Run against the old
  classifier, on messages this program really produces: a missing `scrcpy-server` came
  out as "adb bulunamadı" with a file picker for the adb that had just worked, two
  phones plugged in
  came out as "Cihaz bulunamadı" with advice to plug one in, the `--require-audio` and
  `--no-audio` refusal came out as a warning saying audio needs Android 11, and port
  exhaustion — the one failure the port card exists for — fell through to the generic
  card, because the card's phrases were upstream C scrcpy's rather than this client's.
  The test had been asserting on upstream's wording under a comment saying these are
  messages "this program or adb really produces".
- **The control queue would throw away the finger coming up.** `QUEUE_LIMIT` is
  documented as the limit "for droppable control messages" and nothing in the code knew
  which those were: sixty touches is one drag, and the release that ends it went over the
  side as readily as a move, leaving the device believing the finger was still down. Moves
  and scroll notches give way now; everything else waits a quarter of a second for room,
  and a channel that has actually died comes straight back rather than waiting.
- **One daemon listed the devices while another one mirrored them.** An explicit adb
  port of 5037 — which is what the panel's own field says out of the box — was left off
  the children on the grounds that it is adb's default, which is true only when
  `ANDROID_ADB_SERVER_PORT` is absent from this process's environment. It is inherited.
- **The server's log stopped after five quiet minutes.** The shell stream carried a
  five-minute read timeout, which is a deadline on a quiet shell rather than a slow one,
  and the reader read the timeout as end of stream. Everything the server printed
  afterwards went nowhere.
- **Thirty translations were one `msgcat` away from vanishing.** build.rs read only the
  first line of a `.po` entry, and a wrapped entry — what msgfmt, msgmerge and xgettext
  write for anything past the seventy-ninth column, and 28 of these msgids are longer
  than that — was dropped whole and silent. Running the file through `msgcat` took the
  table from 397 entries to 367. Three messages had drifted out of the file as well and
  were showing in Turkish in an English panel; a test now walks the Rust source for
  `tr!` calls and holds every one to the generated table, and a second walks the error
  cards, whose strings reach `tr!` as variables and are invisible to the first.
- **`--video-buffer` shifts the stream; it does not smooth it.** The file said it
  "smooths out network jitter", and it cannot as written: a frame's release time is its
  arrival plus a constant, so the spacing on the way out is the spacing on the way in.
  Doing what upstream does needs the frame's own timestamp carried through the decoder,
  which `DecodedFrame` does not have. The claim is gone and a test pins what the file
  actually does. Its stop is prompt now as well — it waited in a `sleep` no notification
  could cut short, so a buffer told to stop held its thread, and the join behind it, for
  the rest of the delay: with the sleep put back, the new test measures the join waiting
  2.95 s of a 3 s delay for a buffer that had been told to go.
- **The UHID gamepad introduced itself as nothing in particular.** Vendor and product
  zero, where upstream sends the Xbox 360's — which is what makes Android load a key
  layout that puts the triggers and the right stick where a game looks for them. And an
  analog trigger arriving as an axis was read as though it were 0..1, where every gilrs
  axis is -1..1, so the first half of the pull did nothing. Still the one thing here with
  no pad to try it on: both are read off gilrs's source and upstream's, not off a device.

Two tests were also racing each other rather than testing anything: `cargo test --release
adb::settings` failed 5 runs in 60, because two tests wrote the same process-wide setting
in parallel and a third read it. They take turns now — 60 runs, none failed.


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
  starts, enumerates none, and the session is unaffected — and none of it with one. Two
  things in it were wrong for the same reason: nobody could try it. The pad was created
  with a vendor and product of zero, where upstream sends the Xbox 360's `045e:028e` and
  the name to match — the identity is what makes Android pick a key layout that puts the
  triggers on LTRIGGER and RTRIGGER rather than on the right stick's two axes, and the
  report descriptor here is byte-identical to scrcpy's, so it was the half that gives it
  meaning that had been left behind. And an analog trigger arriving as an *axis* was read
  as though it were 0..1: `axis_value` in gilrs ends `val / range * 2.0 - 1.0`, so a
  released trigger reports -1, and clamping that away made the first half of the pull do
  nothing at all. Pads with an entry in SDL's database were never affected — their
  triggers arrive as buttons, which really are 0..1 — so this is the road every pad the
  database has not heard of takes.
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
  `--v4l2-buffer`. A loopback is told its size once, so a stream that changes size mid
  session is reported rather than written at the wrong stride — and reported *once*, not
  once a frame: changing the phone's display to 720x1600 with a sink attached logs the
  warning a single time and the session carries on. It needs the module first:
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
  1080 wide spills eight pixels past the end of every row and 1081 spills seven, while 64
  spills none — 24 bytes and 21 when this was measured into packed RGB, 32 and 28 now that
  the destination is RGBA. Every row's spill but the last lands in the row below, which is
  written next and covers it; the last row's lands past the end of a buffer that is exactly
  `width * height * 4` bytes, in memory the client does not own. That is not a theoretical complaint: made to
  write straight in at 1080x2400 anyway, this client does not survive the first frame —
  glibc fails an assertion in `sysmalloc` and the process takes SIGABRT. So the last rows are converted into a buffer
  with room to spare and copied in — one or two of them: two when the height is even,
  because a 4:2:0 chroma plane cannot begin on an odd row, which is 8640 bytes of memcpy a
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
  3.98 ms — against 2.96 for decoding and converting the same frame. Handing back the same
  buffer instead costs 0.02, so it is the traffic and not the drawing. `cargo run --release
  --example frame_cost` measures it, six buffers deep because that is the session's frame
  pool. Two renderers linked in changes what the default one is, so the comparisons here
  name theirs — `SLINT_BACKEND=winit-femtovg` against `WGPU=1`.
- **And most of that eight megabytes is not the traffic either — it is the fourth byte.**
  Three bytes a pixel is not a texture format any card has, so somebody pads it out, every
  frame, on the CPU. Handing Slint the same picture already RGBA — 10.4 MB rather than 7.8,
  a third more to carry — costs 0.94 ms a frame against 3.98. swscale converts into RGBA for
  0.48 where RGB24 costs 0.59, four-byte writes suiting it better than three, and the
  decoder's own total is unchanged: switching the destination format inside one binary and
  running it both ways gives 3.1 ms a frame either way, six runs of 568 frames. That
  absolute drifts between sittings — 2.8 to 3.3 over this session — which is why the two
  arms were run back to back in one; what is being claimed is that they move together, not
  the figure. So this is
  the change the client has made, and it needs no shader, no WGPU and no unstable API.
- And it is not only the harness saying so. The same probe put on the mirror window's own
  renderer, in a live session on the Redmi with the screen scrolling: 4.04 ms a draw over
  1500 draws before the change, 0.98 over 1520 after. The before figure came from a build of
  the commit prior to it, made in a worktree so the two differed by nothing else.
- Two things still pay for three bytes a pixel, and both are off by default. `--v4l2-sink`
  publishes RGB24, which V4L2 has a fourcc for, so the fourth byte is dropped on the way
  there — 1.25 ms a frame at 1080x2400, on the thread that pumps frames, and the one place
  left that wants a pass over the picture. It was 1.42 written the obvious way, a pixel at
  a time into a growing buffer; sizing the buffer once and writing over it is the
  difference, and `SWS=1 cargo run --release --example frame_cost` prints both.
  `--display-orientation=flipN` mirrors rows four bytes at a time now instead of three,
  which costs the same as it did.
- The sink has been run since, both ways. `V4L2_DEVICE=/dev/video9 cargo test --release --
  --ignored v4l2` writes a frame in as RGBA and reads it back out of the loopback as RGB24,
  building the picture it expects itself rather than with the function under test — and
  dropping the wrong byte on purpose makes it fail, which is how that was checked to have
  teeth. Both want the module: `sudo modprobe v4l2loopback video_nr=9 card_label=scrcpy
  exclusive_caps=1`.
- Which also gives this machine the one way it has of seeing what the client produces. There
  is no screenshot tool here that can photograph a Slint window — `grim`, `spectacle` and
  `scrot` are absent and ImageMagick's `import -window root` captures nothing — so a claim
  about the pixels normally has to come from Slint's own `take_snapshot`, which returns a
  blank buffer on the OpenGL renderer. A loopback does not: publish the mirror to one, read
  a frame with `ffmpeg -f v4l2`, and compare it against `adb exec-out screencap -p`.
  Cut to the last four rows — the ones `Write::Tail` converts separately, and the only place
  a mistake would hide — that comparison is 744 of 65535 against 475 for four rows out of
  the middle and 393 for the first four. All three are the same order, which is the encoder
  rather than the conversion; what proves the tail exactly is the test that holds it to a
  whole-picture conversion at six sizes, byte for byte.
- **The shader is written and measured, and does not earn its keep — though not for the
  reason this used to give.** `src/ui/yuv.rs` uploads the YUV420P planes as three R8
  textures and converts them in one pass, which comes to 1.25 ms a frame: 0.70 drawing and
  0.55 uploading and converting. Those two are disjoint rather than nested, so a frame costs
  their sum; the harness used to print the second as "of it" and now prints the total. The
  figure did not move when the conversion became a texture load rather than a sample —
  1.25 both times, to two decimal places, over two runs.
- **The number it was being compared against does not exist.** This bullet used to end
  "against the fourth byte's 1.42 that is 0.17 ms", and 1.42 is in no run and contradicts
  this same file, which records 2.52 for the RGBA upload on the WGPU renderer two bullets
  down. Measured again over both renderers in one sitting, from one binary, at 1080x2400:

  | a frame costs | OpenGL renderer | WGPU renderer |
  |---|---|---|
  | packed RGB, a new one every time | 2.93 | 3.93 and 12.60 |
  | the same frame again | 0.01 | 0.65, 0.67 |
  | a new one, already RGBA | 0.91 | 2.39, 2.42 |
  | the planes and the shader | — | 1.25, 1.25 |

  Every figure already written down is confirmed — 0.02 for the OpenGL floor, 0.68 for the
  WGPU one, 0.94 for the RGBA upload, 2.52 for the same upload on WGPU — except 1.42, which
  is the one that was wrong. The packed-RGB row is the only one that will not repeat, and it
  is not load-bearing for anything here.
- **So the ledger reads the other way up, and says the same thing louder.** On the renderer
  the shader needs, it beats handing Slint RGBA by 1.14 ms — but nothing would put the
  client on that renderer for its own sake, and the shader cannot be reached from the one it
  uses. The choice is 1.25 ms with a WGPU renderer, an `unstable-wgpu-29` texture import and
  a `wgpu` dependency, against **0.91 with none of them**. It is 0.34 ms dearer than what
  ships, where this used to claim it was 0.17 cheaper. So it stays behind `--features wgpu`,
  where `frame_cost` uses it and the client does not.
- `frame_cost` will not run with its window occluded: the compositor stops sending frame
  callbacks to a surface nobody can see, and the run hangs rather than failing. Taking these
  needed `env -u WAYLAND_DISPLAY DISPLAY=:1`, which puts it on Xwayland where nothing
  throttles it — and even there the OpenGL renderer stalled twice before a run came back
  whole.
- **It read the wrong chroma sample at every odd width, and the test that would have said so
  did not exist.** Two comments in `src/ui/yuv.rs` named `the_shader_and_swscale_agree` as
  the thing holding it to swscale; there was no such test. What there was is `CHECK=1` in
  `frame_cost`, which printed a mean and a worst and asserted on neither, at a default size
  that is even — and even is exactly where the fault is invisible. The shader sampled all
  three planes with one normalised coordinate, and that coordinate is the *luma* plane's:
  the chroma planes are ceil(w/2) by ceil(h/2), which at an even width covers the same
  picture and at an odd one covers half a chroma texel more, so the same fraction lands a
  texel to the side for every other column from the middle of the picture onwards. Against
  swscale that read a mean of 16.5 of 255 at 1081x2400 where 1080x2400 read 0.70. It loads
  the texel by index now — floor(x/2), floor(y/2), which is what 4:2:0 means — and 1081x2400
  reads what the even size reads. The sampler is gone with it; nothing filters these.
- **And swscale is not the reference at an odd height, which is why the shader is now held
  to the definition instead.** Converting YUV420P to RGB24 unscaled, swscale replicates each
  chroma row down two output rows — what the shader does — only while the chroma plane's
  height doubles exactly into the picture's. At an odd height ceil(h/2) rows have to reach h,
  which is not a doubling, so it builds a vertical scaler and blends two chroma rows instead.
  Measured against libswscale directly, with a flat luma and each chroma row labelled: 8x10
  reads every output row as a pure chroma row, weights 0.00 and 0.99; 8x11 reads 0.293,
  0.849, 0.575, 0.043; 1080x2399 and 640x481 read 0.238 and 0.732 alternating. `SWS_POINT`
  does not put it back — it takes the nearest row of the rescaled grid, still not floor(y/2).
  So an odd height was never a comparison of two implementations of one thing, and holding
  the shader to swscale there was measuring the choice of filter: on a deliberately noisy
  frame, 37.8 of 255. The first of the two tests now holds the shader to a CPU reference of
  the same definition — floor(x/2), floor(y/2), BT.601 limited — at eight sizes, odd ones
  included, and with the mirror on as well as off: mean under 0.5 of 255 and worst 2, which
  is f32 on the card against f64 here and nothing else. The second keeps the cross-check
  against swscale where the two really are doing the same arithmetic.
- **swscale does not always fill the row, either — and this one was not only the shader's.**
  Pre-filling the destination with a sentinel and giving it a row exactly the width of the
  picture — which is what the test did, and what the decoder does every frame — it leaves the
  last `width % 16` columns untouched whenever that is between one and seven. 641 loses one,
  68 loses four, 1079 loses seven; 1080, 1081 and 1082 lose none. Swept over every width from
  8 to 400 and every even one to 1600 with no exception — and then found not to be a property
  of libswscale at all: it belongs to the x86 SIMD converter this machine dispatches to, and
  under `av_force_cpu_flags(0)` the same sweep says odd widths lose one and even widths lose
  none. The two agree at 640, 641, 65 and 1080 and disagree at 68, 1079 and 1081. So the
  reference measures how much of itself is real rather than predicting it, and the client's
  fix does not depend on the rule either. Give the row 32 bytes of slack and every converter
  fills every column.
- **The decoder had been losing those columns for real, and the probe that picks its write
  could not see it.** `choose_write` converts the first frame of each size into a buffer with
  room to spare and reads the room back, twice with a different filling, because the failure
  it was written for is swscale *overrunning* the window's buffer. A converter that writes
  *short* leaves that room alone, comes back as the safest of the three, and gets the direct
  write — straight into the window's buffer, with the columns it declined to fill left
  holding whatever was there before: black on a fresh buffer, the previous frame on a
  recycled one. Proved rather than argued: at 66x64 the direct write leaves columns 64 and 65
  untouched by two conversions with two different fillings, which is
  `the_conversion_fills_every_column` in `src/media/decoder.rs`. It had never shown up because
  every width in the suite — 1080, 1081, 64 — is one of the ones that loses nothing, and
  because the test that would have caught it compares against the same swscale call, so the
  hole was on both sides of its comparison. The same two fillings answer the new question at
  no extra cost: a byte left alone by both runs was never written. There is a fourth write
  now, `Padded`, which converts through a row 64 bytes wider than the picture and copies it in
  a row at a time. `--max-size` cannot reach an affected width, since it rounds down to a
  multiple of eight; `--crop` and `--new-display` pass their size straight through and can.
- Two things that check said and should not have. Slint's own `take_snapshot` returns a
  blank buffer rather than an error on the OpenGL renderer here — and two blanks compare
  equal, so the first version of the check reported a perfect match between paths it had not
  drawn. It refuses anything uniform now, and the on-screen comparisons are run on the
  software and WGPU renderers, which do take snapshots: RGBA against packed RGB is 0.000 of
  255 on both, byte for byte the same picture. And the WGPU renderer is only better at the
  thing it is for — the same RGBA upload costs 2.52 ms there against 0.85 on the OpenGL
  renderer of the same build, and 0.94 on the shipping one.
- **The renderer the shader needs is not free, and is not linked in by default.** Slint's
  texture import wants its WGPU renderer, and asking for it changes what every frame costs,
  not only the ones a shader would touch: drawing a window where nothing changed costs 0.68
  ms a frame there against 0.02 on the OpenGL renderer, and the same eight-megabyte upload
  costs 4.09 against 3.90 — both from the build that has both renderers linked in, which is
  why 3.90 rather than the 3.98 the shipping one reads — with 0.8 ms more CPU behind it,
  which looks like work moved onto threads of its own. At the byte count that matters it comes out the other way: 3.9 MB
  costs 1.91 ms on WGPU against 2.17 on OpenGL. So the switch pays for itself, but only
  once the frames are smaller, and until then it is a tax. That is why `wgpu` is an optional
  feature rather than a dependency — linking the renderer in is enough to make Slint pick
  it, and the 0.68 ms floor arrives with it whether or not anything uses the texture import.
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
- The decoder now emits packed RGBA8 instead of YUV420P planes, because Slint takes pixel
  buffers where SDL took YUV textures — RGBA rather than RGB because three bytes a pixel is
  not a texture format and Slint was padding every frame out to four on the CPU. Its scaler is also rebuilt when the stream changes
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
7. ~~GPU frame path~~ — done, and it turned out not to want a GPU. The CPU copies went when
   swscale was pointed at Slint's own buffer; the rest of it was a fourth byte. Packed RGB
   is not a texture format any card has, so something padded it out every frame: at
   1080x2400 that cost 3.98 ms a frame against 0.94 for handing Slint the same picture
   already RGBA — a third more bytes, a quarter of the time. The decoder converts into RGBA
   now, which swscale does for 0.48 against RGB24's 0.59 and which leaves the decoder's own
   cost where it was, 3.1 ms a frame either way. **3.0 ms a frame, off the thread that draws
   and takes input** — 4.04 ms a draw against 0.98 in a live session on the Redmi, measured
   both ways with the same probe. The shader that was meant to be this item is written and
   measured and comes to 1.25 ms against the fourth byte's 0.91 on the renderer that ships —
   it is 0.34 ms *dearer*, because the WGPU renderer it needs taxes every frame more than the
   shader saves — so it lives behind `--features wgpu` and the client does not use it.
   `cargo run --release --example frame_cost` is the measurement
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
