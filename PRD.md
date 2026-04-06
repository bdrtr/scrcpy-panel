# Product Requirements Document (PRD): scrcpyrust

## 1. Project Overview
**scrcpyrust** is a high-performance, memory-safe Rust implementation of the `scrcpy` client. It aims to provide full feature parity with the official C client while leveraging Rust's safety and modern ecosystem.

## 2. Core Requirements

### 2.1 Connection & Tunneling
- [x] **ADB Integration**: Direct implementation of the ADB wire protocol.
- [x] **Automatic Tunneling**: Setting up reverse/forward tunnels via ADB.
- [x] **Wireless Support**: Support for `--tcpip` connection mode.
- [x] **Server Management**: Automatic deployment and execution of `scrcpy-server.jar`.

### 2.2 Video Streaming
- [x] **Codecs**: Support for H.264, H.265, and AV1.
- [x] **Decoding**: Hardware-accelerated decoding via FFmpeg (D3D11VA, VAAPI, etc.).
- [x] **Rendering**: SDL2-based low-latency YUV rendering.
- [x] **Jitter Buffer**: Configurable video buffer for smooth playback.
- [x] **Camera Mirroring**: Support for mirroring device cameras as a video source.

### 2.3 Audio Streaming
- [x] **Codecs**: Support for Opus, AAC, FLAC, and Raw.
- [x] **Playback**: SDL2 audio playback with drift compensation.
- [x] **Regulator**: Dynamic resampling to maintain buffer stability.

### 2.4 Input Control
- [x] **SDK Mode**: Standard keycode/touch injection.
- [x] **UHID Keyboard**: Virtual HID keyboard for better compatibility and international support.
- [x] **UHID Mouse**: Relative and absolute mouse control via HID.
- [x] **UHID Gamepad**: Virtual HID gamepad support (up to 4 controllers).
- [x] **Shortcuts**: Full set of Mod-key shortcuts (Home, Back, Power, Volume, etc.).
- [x] **Pinch-to-Zoom**: Virtual multi-touch simulation.

### 2.5 Recording
- [x] **Muxing**: Real-time recording to MP4, MKV, M4A, and MKA.
- [x] **Audio-Only**: Support for recording audio without video.

### 2.6 System Integration
- [x] **Clipboard**: Bi-directional auto-sync between PC and device.
- [x] **File Push**: Drag-and-drop file installation/upload via ADB SYNC.
- [x] **V4L2**: Virtual webcam output for Linux (webcam emulation).

## 3. Technical Specifications
- **Language**: Rust (Stable)
- **Primary Dependencies**: `sdl2`, `ffmpeg-next`, `clap`, `anyhow`, `crossbeam-channel`.
- **Target OS**: Windows, Linux (macOS support planned).
- **Parity Target**: Genymobile/scrcpy v2.4+

## 4. Performance Goals
- **Latency**: Sub-50ms glass-to-glass (network dependent).
- **Efficiency**: Low CPU usage via hardware decoding and lock-free threading.
- **Robustness**: Memory safety guaranteed by Rust; graceful handling of ADB disconnections.
