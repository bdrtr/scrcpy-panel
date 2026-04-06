# Master Prompt: Recreating scrcpyrust

> [!TIP]
> **Prompt Instruction**: "Act as a senior Rust systems engineer. Your task is to build a high-performance Android mirroring client (`scrcpyrust`) from scratch, compatible with the official `scrcpy-server.jar`."

## 1. Core Requirements
1. **ADB Bridge**: Implement the ADB wire protocol (CNXN, OPEN, WRTE, OKAY, CLSE) in pure Rust. Use it to push `scrcpy-server.jar` and establish TCP tunnels (reverse/forward).
2. **Server Handshake**: Connect to the server's video, audio, and control sockets. Handle initial headers for device name, codec selection, and display info.
3. **Media Pipeline**: Use `ffmpeg-sys-next` for low-level HW-accelerated video decoding. Use `sdl2` for YUV/RGB texture rendering and audio playback with drift-compensated regulation.
4. **Input Control**: Support standard SDK injection and advanced UHID keyboard/mouse/gamepad simulation. Implement the full scrcpy control message protocol (18 message types).
5. **Recording**: Mux video/audio packets into MP4/MKV in real-time with monotonic PTS normalization.

## 2. Technical Hurdles & Solutions
- **HW Acceleration**: Implement the `get_format` callback in FFmpeg to negotiate the correct HW pixel format (D3D11, VAAPI, etc.).
- **Jitter Handling**: Create a thread-safe `DelayBuffer` using `VecDeque` and `Arc<Mutex>` to buffer frames for N milliseconds.
- **Audio Tuning**: Implement a ring-buffer regulator that dynamically resamples audio to keep the buffer at an ideal ~50ms level, preventing audio crackling/delay.
- **Binary Protocols**: Be precise with LE (little-endian) for ADB headers and BE (big-endian) for scrcpy video/audio/control headers.

## 3. Recommended Dependency Stack
- `sdl2`: Cross-platform window/render/audio.
- `ffmpeg-next` / `ffmpeg-sys-next`: Media decoding and muxing.
- `clap`: Command-line arg parsing with 70+ flags.
- `anyhow`: Robust error propagation.
- `crossbeam-channel`: High-throughput, lock-free communication.

## 4. Architectural Goal
Achieve 100% feature parity with `Genymobile/scrcpy` (version 2.4+) while maintaining a clean, modular Rust codebase of approximately 6,000-8,000 lines. Focus on memory safety and data-race prevention.
