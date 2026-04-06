$ADB = "C:\Users\Nacer\AppData\Local\Android\Sdk\platform-tools\adb.exe"
$RUST_EXE = "e:\projects1\scrcpyrust\target\release\scrcpyrust.exe"
$RUST_DIR = "e:\projects1\scrcpyrust\target\release"
$C_EXE = "E:\CODEX\ScrpyGui\runtime\scrcpy-win64-v3.3.4\scrcpy.exe"
$C_DIR = "E:\CODEX\ScrpyGui\runtime\scrcpy-win64-v3.3.4"
$DURATION = 15

& $ADB devices

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host " BENCHMARK: Rust scrcpyrust vs C scrcpy" -ForegroundColor Cyan  
Write-Host " Duration: ${DURATION}s each" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# --- Benchmark Rust build ---
Write-Host ">>> Starting RUST build..." -ForegroundColor Yellow
$env:RUST_LOG = "warn"
$rustStart = Get-Date
$rustProc = Start-Process -FilePath $RUST_EXE -ArgumentList "--no-audio","--turn-screen-off" -WorkingDirectory $RUST_DIR -PassThru
Start-Sleep -Seconds 4

$rustSamples = @()
for ($i = 0; $i -lt $DURATION; $i++) {
    Start-Sleep -Seconds 1
    try {
        $p = Get-Process -Id $rustProc.Id -ErrorAction Stop
        $cpu = $p.CPU
        $mem = [math]::Round($p.WorkingSet64 / 1MB, 1)
        $pvm = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        $threads = $p.Threads.Count
        $rustSamples += [PSCustomObject]@{Sec=$i; CPU=$cpu; MemMB=$mem; PrivMB=$pvm; Threads=$threads}
    } catch { Write-Host "  Rust process ended early at ${i}s"; break }
}

try { $rustProc | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
$rustElapsed = ((Get-Date) - $rustStart).TotalSeconds
Start-Sleep -Seconds 3

Write-Host ">>> Rust build stopped after $([math]::Round($rustElapsed))s`n" -ForegroundColor Yellow

# --- Benchmark C build ---
Write-Host ">>> Starting C build..." -ForegroundColor Green
$cStart = Get-Date
$cProc = Start-Process -FilePath $C_EXE -ArgumentList "--no-audio","--turn-screen-off" -WorkingDirectory $C_DIR -PassThru
Start-Sleep -Seconds 4

$cSamples = @()
for ($i = 0; $i -lt $DURATION; $i++) {
    Start-Sleep -Seconds 1
    try {
        $p = Get-Process -Id $cProc.Id -ErrorAction Stop
        $cpu = $p.CPU
        $mem = [math]::Round($p.WorkingSet64 / 1MB, 1)
        $pvm = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        $threads = $p.Threads.Count
        $cSamples += [PSCustomObject]@{Sec=$i; CPU=$cpu; MemMB=$mem; PrivMB=$pvm; Threads=$threads}
    } catch { Write-Host "  C process ended early at ${i}s"; break }
}

try { $cProc | Stop-Process -Force -ErrorAction SilentlyContinue } catch {}
$cElapsed = ((Get-Date) - $cStart).TotalSeconds
Start-Sleep -Seconds 2

Write-Host ">>> C build stopped after $([math]::Round($cElapsed))s`n" -ForegroundColor Green

# --- Results ---
$rustSize = [math]::Round((Get-Item $RUST_EXE).Length / 1KB)
$cSize = [math]::Round((Get-Item $C_EXE).Length / 1KB)

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " RESULTS" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Write-Host "`n--- Binary Size ---"
Write-Host ("  Rust:  {0,6} KB  ({1} MB)" -f $rustSize, [math]::Round($rustSize/1024,1))
Write-Host ("  C:     {0,6} KB  ({1} MB)" -f $cSize, [math]::Round($cSize/1024,1))

if ($rustSamples.Count -gt 2) {
    $rustAvgMem = [math]::Round(($rustSamples | Measure-Object -Property MemMB -Average).Average, 1)
    $rustMaxMem = [math]::Round(($rustSamples | Measure-Object -Property MemMB -Maximum).Maximum, 1)
    $rustAvgPriv = [math]::Round(($rustSamples | Measure-Object -Property PrivMB -Average).Average, 1)
    $rustCpuDelta = [math]::Round($rustSamples[-1].CPU - $rustSamples[0].CPU, 2)
    $rustAvgCpu = [math]::Round($rustCpuDelta / $rustSamples.Count * 100, 1)
    $rustAvgThreads = [math]::Round(($rustSamples | Measure-Object -Property Threads -Average).Average)

    Write-Host "`n--- Rust scrcpyrust ---"
    Write-Host ("  Working Set:  {0} MB avg / {1} MB peak" -f $rustAvgMem, $rustMaxMem)
    Write-Host ("  Private Mem:  {0} MB avg" -f $rustAvgPriv)
    Write-Host ("  CPU time:     {0}s over {1}s wall" -f $rustCpuDelta, $DURATION)
    Write-Host ("  CPU usage:    ~{0}%" -f $rustAvgCpu)
    Write-Host ("  Threads:      {0}" -f $rustAvgThreads)
} else {
    Write-Host "`n--- Rust: not enough samples ---"
}

if ($cSamples.Count -gt 2) {
    $cAvgMem = [math]::Round(($cSamples | Measure-Object -Property MemMB -Average).Average, 1)
    $cMaxMem = [math]::Round(($cSamples | Measure-Object -Property MemMB -Maximum).Maximum, 1)
    $cAvgPriv = [math]::Round(($cSamples | Measure-Object -Property PrivMB -Average).Average, 1)
    $cCpuDelta = [math]::Round($cSamples[-1].CPU - $cSamples[0].CPU, 2)
    $cAvgCpu = [math]::Round($cCpuDelta / $cSamples.Count * 100, 1)
    $cAvgThreads = [math]::Round(($cSamples | Measure-Object -Property Threads -Average).Average)

    Write-Host "`n--- C scrcpy (official v3.3.4) ---"
    Write-Host ("  Working Set:  {0} MB avg / {1} MB peak" -f $cAvgMem, $cMaxMem)
    Write-Host ("  Private Mem:  {0} MB avg" -f $cAvgPriv)
    Write-Host ("  CPU time:     {0}s over {1}s wall" -f $cCpuDelta, $DURATION)
    Write-Host ("  CPU usage:    ~{0}%" -f $cAvgCpu)
    Write-Host ("  Threads:      {0}" -f $cAvgThreads)
} else {
    Write-Host "`n--- C: not enough samples ---"
}

Write-Host "`n========================================" -ForegroundColor Cyan
