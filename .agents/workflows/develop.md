---
description: How to add new features to scrcpyrust
---

# Adding Features

## Adding a New Control Message

1. Add the message type constant in `src/control/control_msg.rs`
2. Add a new variant to the `ControlMsg` enum
3. Implement serialization in the `serialize()` method
4. Add handler in `src/input/manager.rs` to trigger it from SDL events

## Adding a New Keyboard Shortcut

1. Add the action to `ShortcutAction` enum in `src/input/shortcuts.rs`
2. Map the key in `get_shortcut()` function
3. Handle the action in `src/input/manager.rs` → `handle_event()`

## Adding Audio Support

1. Read the audio header in `main.rs` using `demuxer::read_audio_header()`
2. Create an audio decoder similar to `VideoDecoder` but for audio codecs
3. Use `sdl2::audio` to create an audio device and stream
4. Feed decoded audio samples to the SDL audio callback

## Adding Recording

1. Create `src/media/recorder.rs`
2. Use `ffmpeg-next` to create an output format context
3. Register as a consumer of demuxer packets (before decoding)
4. Write packets with rescaled timestamps to the output file

## Wire Protocol Reference

### Packet Header (12 bytes)
```
Bytes 0-7: PTS + flags (big-endian u64)
  Bit 63: config packet flag
  Bit 62: key frame flag
  Bits 0-61: PTS value
Bytes 8-11: packet data length (big-endian u32)
```

### Codec IDs (4-byte ASCII)
- `h264` = 0x68323634
- `h265` = 0x68323635
- `av1`  = 0x00617631
- `opus` = 0x6f707573
- `aac`  = 0x00616163
