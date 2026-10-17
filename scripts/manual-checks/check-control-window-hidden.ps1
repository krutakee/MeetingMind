# Acceptance check: the control window does not appear in the recorded video.
#
# Runs the real app, starts a recording with the global shortcut, paints the control window solid green, records for a
# few seconds, stops, and scans the saved video for green in the place where the control window was on the screen.
# Needs a Windows desktop (a real monitor), a built debug app (cargo build in src-tauri) and Node. It records the screen
# and the microphone for a few seconds; the recording is deleted at the end.
#
#   powershell -File scripts\manual-checks\check-control-window-hidden.ps1
#
# Do not use the keyboard while it runs: it sends Ctrl+Shift+S (the start/stop shortcut) and Enter (the consent dialog).

param(
    [int]$CdpPort = 9347,
    [int]$RecordSeconds = 8
)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path "$here\..\..").Path
$exe = "$repo\src-tauri\target\debug\meetingmind.exe"
$ffmpeg = "$repo\src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe"
. "$here\keys.ps1"
. "$here\windows-of.ps1"
$env:CDP_PORT = "$CdpPort"

function Fail($message) { Write-Host "FAIL: $message" -ForegroundColor Red; $script:failed = $true }
function Info($message) { Write-Host $message }

if (-not (Test-Path $exe)) { throw "build the app first: cargo build (in src-tauri). Missing $exe" }
if (-not (Test-Path $ffmpeg)) { throw "install FFmpeg first: pnpm fetch:ffmpeg. Missing $ffmpeg" }
if (Get-NetTCPConnection -LocalPort $CdpPort -State Listen -ErrorAction SilentlyContinue) { throw "port $CdpPort is already in use; pass another with -CdpPort" }
if (Get-Process meetingmind -ErrorAction SilentlyContinue) { throw "MeetingMind is already running; close it first" }

# 1. The scanner must find green when it is there (otherwise a pass would mean nothing).
Info "1. scanner self-test"
node "$here\scan-frames-for-colour.mjs" $ffmpeg --self-test
if ($LASTEXITCODE -ne 0) { throw "the scanner self-test failed" }

# 2. Start the web page server if needed, then the app, with a debugging port of its own.
$startedVite = $null
if (-not (Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue)) {
    Info "2. starting the web page server"
    $startedVite = Start-Process -FilePath "pnpm.cmd" -ArgumentList "dev" -WorkingDirectory $repo -WindowStyle Hidden -PassThru
    for ($i = 0; $i -lt 40 -and -not (Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
}
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
$app = Start-Process -FilePath $exe -WorkingDirectory "$repo\src-tauri" -PassThru
$script:failed = $false
$recordingFolder = $null
try {
    Info "   waiting for the app's main window"
    $ready = $false
    for ($i = 0; $i -lt 90 -and -not $ready; $i++) {
        Start-Sleep -Seconds 1
        try { $ready = (node "$here\cdp.mjs" main eval "document.querySelector('h1') !== null" 2>$null) -eq 'true' } catch { $ready = $false }
    }
    if (-not $ready) { throw "the app's main window did not come up" }
    Start-Sleep -Seconds 3

    # 3. Start recording with the shortcut; answer the consent dialog if it appears.
    Info "3. starting a recording with Ctrl+Shift+S"
    Press-CtrlShift $VK_S
    $control = $null
    for ($i = 0; $i -lt 120 -and -not $control; $i++) {
        Start-Sleep -Milliseconds 500
        $windows = [WindowList]::Of([uint32]$app.Id)
        if ($windows | Where-Object { $_ -match 'class=#32770' -and $_ -match 'visible=True' }) { Info "   consent dialog: pressing Enter"; Press-Enter; Start-Sleep -Seconds 1 }
        $control = $windows | Where-Object { $_ -match "title='MeetingMind recording'" -and $_ -match 'visible=True' } | Select-Object -First 1
    }
    if (-not $control) { throw "the control window did not appear" }

    # 4. Paint it solid green, and note where it is.
    node "$here\cdp.mjs" control eval "document.documentElement.style.background = '#00ff00'; document.body.innerHTML = ''; document.body.style.background = '#00ff00'; getComputedStyle(document.body).backgroundColor" | Out-Null
    Start-Sleep -Seconds 1
    $painted = node "$here\cdp.mjs" control eval "getComputedStyle(document.body).backgroundColor"
    $control = [WindowList]::Of([uint32]$app.Id) | Where-Object { $_ -match "title='MeetingMind recording'" } | Select-Object -First 1
    Info "   control window: $control"
    Info "   page colour: $painted"
    if ($painted -notmatch '0, 255, 0') { Fail "the control window page is not green" }
    if ($control -notmatch 'visible=True') { Fail "the control window is not visible on the screen" }
    if ($control -notmatch 'affinity=0x11') { Fail "the control window is not excluded from capture (affinity $control)" }
    if ($control -notmatch 'size=(\d+)x(\d+) at=\((-?\d+),(-?\d+)\)') { throw "cannot read the control window position" }
    $w = [int]$Matches[1]; $h = [int]$Matches[2]; $x = [int]$Matches[3]; $y = [int]$Matches[4]

    Info "   recording for $RecordSeconds s with the green control window on the screen"
    Start-Sleep -Seconds $RecordSeconds

    # 5. Stop, wait for the saved recording.
    Info "5. stopping with Ctrl+Shift+S"
    Press-CtrlShift $VK_S
    $saved = $false
    for ($i = 0; $i -lt 90 -and -not $saved; $i++) {
        Start-Sleep -Seconds 1
        try { $saved = (node "$here\cdp.mjs" main eval "document.body.innerText.includes('Recording saved')" 2>$null) -eq 'true' } catch { $saved = $false }
    }
    if (-not $saved) { throw "the recording was not saved" }
    $downloads = (New-Object -ComObject Shell.Application).NameSpace('shell:Downloads').Self.Path
    $recordingFolder = (Get-ChildItem "$downloads\MeetingMind" -Directory | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
    $video = "$recordingFolder\recording.mp4"
    Info "   saved: $video"

    # 6. Scan the video where the window was. The monitor is the main one, at the origin, so the screen position is the
    #    video position (the video is the monitor's own size at the default 1080p setting; scaled otherwise).
    $videoWidth = [int](Get-Content (Join-Path $recordingFolder 'meeting.json') -Raw | ConvertFrom-Json).video.width
    Add-Type -AssemblyName System.Windows.Forms
    $scale = $videoWidth / [double]([System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width)
    $sx = [int]($x * $scale); $sy = [int]($y * $scale); $sw = [int]($w * $scale); $sh = [int]($h * $scale)
    Info "6. scanning the video for green in x=$sx y=$sy width=$sw height=$sh"
    node "$here\scan-frames-for-colour.mjs" $ffmpeg $video $sx $sy $sw $sh 0 255 0 40
    $found = $LASTEXITCODE
    if ($found -eq 0) { Info "   no frame has green where the control window was" } else { Fail "green was found in the video where the control window was" }
}
finally {
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
    if ($startedVite) { Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.OwningProcess -Force -ErrorAction SilentlyContinue } }
    Info "cleanup: the test recording is $recordingFolder"
    Info "         delete that folder (and its entry in the app's list, see scripts\manual-checks\README.md)"
}
if ($script:failed) { Write-Host "RESULT: FAILED" -ForegroundColor Red; exit 1 }
Write-Host "RESULT: PASSED - the green control window was on the screen during the recording and is not in the video" -ForegroundColor Green
