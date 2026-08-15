# scrcpyrust — Brain Dump & Knowledge Base

## What Is This Project?

scrcpyrust is a **complete Rust rewrite** of the [scrcpy](https://github.com/Genymobile/scrcpy) Android screen mirroring client. It talks to the **same Java server** (scrcpy-server.jar) that the official C client uses, making it a drop-in replacement.

## Core Concepts

### 1. The Client-Server Split
scrcpy has two parts:
- **Server** (Java): Runs on the Android device. Captures the screen using Android's MediaCodec, encodes to H.264/H.265/AV1, and sends raw packets over a TCP socket. Also handles input injection (touch, keyboard, clipboard).
- **Client** (our Rust code): Runs on your PC. Receives encoded video packets, decodes them with FFmpeg, and renders them in an SDL2 window. Captures keyboard/mouse input and sends it back to the server.

### 2. The ADB Bridge
The client and server communicate through ADB (Android Debug Bridge):
1. Client pushes `scrcpy-server.jar` to `/data/local/tmp/` on the device
2. Client sets up a TCP tunnel (reverse or forward) through ADB
3. Client starts the server via `adb shell` with configuration parameters
4. Server connects back through the tunnel
5. Three sockets are established: video, audio, control

### 3. The Pipeline
```
[Android Screen] → [MediaCodec Encode] → [TCP Socket] → [FFmpeg Decode] → [SDL2 Render] → [Your Monitor]
[Your Keyboard/Mouse] → [SDL2 Events] → [Control Messages] → [TCP Socket] → [Android InputManager]
```

## Key Technical Details

### ADB Wire Protocol
ADB uses a simple binary protocol:
- Each message has a 24-byte header: `command(4) + arg0(4) + arg1(4) + length(4) + checksum(4) + magic(4)`
- Commands: CNXN (connect), OPEN (open stream), WRTE (write data), OKAY (acknowledge), CLSE (close)
- All integers are little-endian

### Video Packet Format
The server sends packets in this format:
- 12-byte header: `pts(8, big-endian microseconds) + size(4, big-endian bytes)`
- Followed by `size` bytes of encoded video data
- First packet has pts=0 and contains codec config (SPS/PPS for H.264)

### Control Message Format
Messages sent from client to server:
- 1 byte: message type (0-17)
- Variable payload depending on type
- All multi-byte values are big-endian

### UHID Protocol
For UHID keyboard/mouse/gamepad, the client creates virtual HID devices on the Android device:
- Sends a UHID_CREATE message with the HID report descriptor
- Sends UHID_INPUT messages with HID reports for key/mouse/gamepad events
- Receives UHID_OUTPUT messages for LED state changes (NumLock, CapsLock)

## File Organization Mental Model

```
src/
├── main.rs           ← "The brain" — orchestrates everything
├── options.rs        ← "The configuration" — 71 CLI flags
├── adb/              ← "The bridge" — talks to ADB
│   ├── protocol.rs   ← Low-level ADB wire protocol
│   ├── commands.rs   ← High-level ADB operations
│   ├── tunnel.rs     ← Port tunneling (reverse/forward)
│   └── sync.rs       ← File push protocol
├── server/           ← "The launcher" — starts the server
│   ├── params.rs     ← Builds server command-line args
│   └── connection.rs ← TCP handshake with server
├── media/            ← "The decoder" — handles video/audio
│   ├── demuxer.rs    ← Splits raw stream into packets
│   ├── decoder.rs    ← FFmpeg video decoder (HW accel)
│   ├── audio_decoder.rs ← FFmpeg audio decoder
│   ├── recorder.rs   ← Records to MP4/MKV/M4A
│   └── delay_buffer.rs ← Jitter compensation buffer
├── display/          ← "The screen" — shows video
│   ├── screen.rs     ← SDL2 window + texture rendering
│   ├── fps_counter.rs ← FPS tracking
│   └── v4l2_sink.rs  ← Linux webcam emulation (stub)
├── audio/            ← "The speaker" — plays audio
│   ├── player.rs     ← SDL2 audio playback
│   └── regulator.rs  ← Drift compensation
├── input/            ← "The controller" — handles input
│   ├── manager.rs    ← Event dispatch + shortcuts
│   ├── hid_keyboard.rs ← UHID keyboard HID reports
│   ├── hid_mouse.rs  ← UHID mouse HID reports
│   ├── hid_gamepad.rs ← UHID gamepad HID reports
│   ├── aoa_hid.rs    ← AOA USB HID (stub)
│   ├── keymap.rs     ← Scancode → keycode tables
│   └── shortcuts.rs  ← Mod+key bindings
├── control/          ← "The messenger" — sends commands
│   ├── control_msg.rs ← Wire protocol serialization
│   ├── controller.rs  ← Message queue + sender
│   └── device_msg.rs  ← Incoming device messages
└── util/             ← "The toolbox" — helpers
    ├── binary.rs     ← Byte read/write
    └── net.rs        ← TCP socket helpers
```

## Common Patterns Used

### 1. Channel-Based Pipeline
```rust
let (tx, rx) = crossbeam_channel::bounded(8);
// Demuxer thread sends packets via tx
// Decoder thread receives via rx
// This naturally handles backpressure
```

### 2. Arc<Mutex<T>> for Shared State
```rust
let recorder = Arc::new(Mutex::new(RecorderState { ... }));
// Multiple threads can push video/audio packets
// Recorder thread drains and muxes them
```

### 3. FFmpeg unsafe FFI
```rust
unsafe {
    let codec = ffi::avcodec_find_decoder(codec_id);
    let ctx = ffi::avcodec_alloc_context3(codec);
    ffi::avcodec_open2(ctx, codec, std::ptr::null_mut());
}
```

### 4. Server Param Builder
```rust
if opts.video_source != "display" {
    args.push(format!("video_source={}", opts.video_source));
}
// Only send non-default values to minimize server args
```

## Known Gotchas

1. **FFmpeg memory**: AVPacket and AVFrame must be freed with `av_packet_free`/`av_frame_free`, not `free()`.
2. **ADB integers**: Wire protocol uses LE, but scrcpy packet headers use BE.
3. **SDL2 threading**: SDL events must be handled on the main thread.
4. **UHID reports**: Must be byte-identical to what Android expects — wrong bit order = silent failure.
5. **Recorder PTS**: Must be monotonically increasing — the recorder manually fixes non-monotonic timestamps.
