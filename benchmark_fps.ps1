$ADB = "C:\Users\Nacer\AppData\Local\Android\Sdk\platform-tools\adb.exe"
$RUST_EXE = "e:\projects1\scrcpyrust\target\release\scrcpyrust.exe"
$RUST_DIR = "e:\projects1\scrcpyrust\target\release"
$C_EXE = "E:\CODEX\ScrpyGui\runtime\scrcpy-win64-v3.3.4\scrcpy.exe"
$C_DIR = "E:\CODEX\ScrpyGui\runtime\scrcpy-win64-v3.3.4"
$DURATION = 15

& $ADB devices

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host " BENCHMARK FPS: Rust scrcpyrust vs C scrcpy" -ForegroundColor Cyan  
Write-Host " Duration: ${DURATION}s each" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# --- Benchmark Rust build ---
Write-Host ">>> Starting RUST build..." -ForegroundColor Yellow
$env:RUST_LOG = "info"
$rustLog = "e:\projects1\scrcpyrust\rust_fps.log"
$rustProc = Start-Process -FilePath $RUST_EXE -ArgumentList "--no-audio","--print-fps" -WorkingDirectory $RUST_DIR -RedirectStandardError $rustLog -PassThru
Start-Sleep -Seconds $DURATION

try { $rustProc | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
Start-Sleep -Seconds 3

Write-Host ">>> Rust build stopped`n" -ForegroundColor Yellow

# --- Benchmark C build ---
Write-Host ">>> Starting C build..." -ForegroundColor Green
$cLog = "e:\projects1\scrcpyrust\c_fps.log"
$cProc = Start-Process -FilePath $C_EXE -ArgumentList "--no-audio","--print-fps" -WorkingDirectory $C_DIR -RedirectStandardOutput $cLog -PassThru
Start-Sleep -Seconds $DURATION

try { $cProc | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
Start-Sleep -Seconds 2

Write-Host ">>> C build stopped`n" -ForegroundColor Green

# --- Results ---
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " FPS RESULTS" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Write-Host "`n--- Rust scrcpyrust ---"
$rustFpsLines = Get-Content $rustLog -ErrorAction SilentlyContinue | Select-String -Pattern "fps"
$rustFps = @()
foreach ($line in $rustFpsLines) {
    if ($line -match "(\d+(\.\d+)?) fps") {
        $rustFps += [double]$matches[1]
    }
}
if ($rustFps.Count -gt 0) {
    $avgFps = [math]::Round(($rustFps | Measure-Object -Average).Average, 1)
    Write-Host ("  Avg FPS:  {0}" -f $avgFps)
    Write-Host ("  Samples:  {0}" -f $rustFps.Count)
    Write-Host ("  All FPS recorded:")
    Write-Host ($rustFps -join ", ")
} else {
    Write-Host "  No FPS data found in Rust."
}

Write-Host "`n--- C scrcpy (official v3.3.4) ---"
$cFpsLines = Get-Content $cLog -ErrorAction SilentlyContinue | Select-String -Pattern "fps"
$cFps = @()
foreach ($line in $cFpsLines) {
    if ($line -match "(\d+) fps") {
        $cFps += [double]$matches[1]
    }
}
if ($cFps.Count -gt 0) {
    $avgFps = [math]::Round(($cFps | Measure-Object -Average).Average, 1)
    Write-Host ("  Avg FPS:  {0}" -f $avgFps)
    Write-Host ("  Samples:  {0}" -f $cFps.Count)
    Write-Host ("  All FPS recorded:")
    Write-Host ($cFps -join ", ")
} else {
    Write-Host "  No FPS data found in C."
}

Write-Host "`n========================================" -ForegroundColor Cyan
