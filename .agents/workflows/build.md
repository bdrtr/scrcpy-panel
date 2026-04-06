---
description: How to build and run scrcpyrust from source
---

# Build and Run

## Prerequisites
- Rust 1.70+ (`rustup` recommended)
- MSYS2 with FFmpeg: `pacman -S mingw-w64-x86_64-ffmpeg`
- ADB in PATH or at `%LOCALAPPDATA%\Android\sdk\platform-tools\`
- scrcpy-server binary (download from [releases](https://github.com/Genymobile/scrcpy/releases))

## Build Steps

// turbo-all

1. Set FFmpeg paths for Windows:
```powershell
$env:FFMPEG_DIR = "D:\msys64\mingw64"
$env:PATH = "D:\msys64\mingw64\bin;$env:LOCALAPPDATA\Android\sdk\platform-tools;$env:PATH"
```

2. Build release binary:
```powershell
cargo build --release --manifest-path e:\projects1\scrcpyrust\Cargo.toml
```

3. Copy scrcpy-server next to binary:
```powershell
Copy-Item e:\projects1\scrcpy\scrcpy-server e:\projects1\scrcpyrust\target\release\scrcpy-server
```

4. Run:
```powershell
e:\projects1\scrcpyrust\target\release\scrcpyrust.exe
```

## Debug Build
```powershell
$env:RUST_LOG = "debug"
cargo run --manifest-path e:\projects1\scrcpyrust\Cargo.toml
```

## Common Issues

- **"FFmpeg not found"**: Ensure `FFMPEG_DIR` points to your MSYS2 mingw64 directory
- **"No device connected"**: Check `adb devices` shows your phone
- **"scrcpy-server not found"**: Place the server binary next to the executable
