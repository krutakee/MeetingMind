# Phase 2 plan — Windows recording

**Status: approved 2026-10-16. M0 (spikes) is done; its results are in [phase-2-spikes.md](phase-2-spikes.md) and are
applied below. M1 and M2 are done (Section 8). Next: M3. Hotkeys changed in review: see Section 10.**
Scope and acceptance criteria come from SPEC.md Sections 5 and 11 and do not change here. This document says how
they will be met, what was checked, what is still unverified, and what needs a decision.

## 1. What Phase 2 delivers

Record on Windows 11, screen + audio or audio only, with pause/resume/stop/cancel, crash-safe files, own-window
exclusion, sleep prevention, device-change handling, disk-space guards, a control window, hotkeys, tray icon, consent
reminder, recovery of unfinished recordings, and basic playback.

Not in Phase 2: transcription (Phase 3), final pass / exports / library / editing (Phase 4), macOS (Phase 5).
The Record screen's setup omits the transcription fields until Phase 3. The audio path is built so Phase 3 can add a
second consumer of the same audio without touching the recording code, but no consumer is written now.

## 2. What was checked (2026-10-16)

| Question | Finding | Status |
|---|---|---|
| Mic and system-audio library | `cpal` 0.18.2, Apache-2.0, updated 2026-08. Its WASAPI code sets `AUDCLNT_STREAMFLAGS_LOOPBACK` (`src/host/wasapi/device.rs`), so one crate can capture mic and system audio. `wasapi` 0.26.0 (MIT, updated 2026-10-11) is the fallback if cpal's timestamps or error reporting are not good enough. | Verified in source; behaviour unverified until spike |
| Screen capture | `windows-capture` 2.0.1, MIT, updated 2026-08 (Windows.Graphics.Capture). | Verified exists and is maintained; behaviour unverified until spike |
| Live FLAC writing | `flacenc` 0.5.1 only documents encoding a whole buffer, not incremental streaming. Not suitable for a live, crash-safe track. FFmpeg's `flac` encoder will write the tracks instead. | Verified (README) |
| FFmpeg LGPL build | BtbN publishes `win64-lgpl` and `win64-lgpl-shared` builds (releases n8.1, n9.0, master). The static LGPL zip is 164-170 MB. A custom minimal build would be much smaller. | Verified exists; contents unverified |
| Encoders in that build | `h264_mf`, `libopenh264`, `aac`, `flac` are present; the build is LGPL v3 with no `--enable-gpl` / `--enable-nonfree`. | Verified in M0 |
| Tauri plugins | `tauri-plugin-global-shortcut` 2.4.1, `tauri-plugin-single-instance` 2.5.4 (latest stable; the crates also have 3.0 alphas, not used). Tray icon is a Tauri core feature. | Verified |
| This machine | Windows 11 Enterprise build 26200; Core Ultra 7 265U (14 threads); 31.5 GB RAM; Intel integrated graphics; 3 monitors (1920x1080, 1920x1080, 1536x960); only built-in speakers and microphone array (no external mic or headset); 192 GB free on C:. | Verified |

**This machine is faster than the SPEC reference machine** (Core i5 / Ultra 5 or Ryzen 5, 16 GB). Numbers measured
here are reported as measured on IV-LT-Han. Nothing will be claimed as a reference-machine result unless it is run on
one.

## 3. Architecture

OS-specific code stays in `capture`, `audio` and `system`, behind traits (SPEC Section 3). Everything else is
identical on Windows and macOS. macOS builds in CI must still compile; the macOS implementations report
"not supported yet" until Phase 5.

| Module | Role | OS-specific? |
|---|---|---|
| `capture` | `ScreenSource` trait; Windows implementation on Windows.Graphics.Capture. Lists monitors, delivers frames with a capture time. | yes |
| `audio` | `AudioSource` trait; Windows implementation on cpal. Lists devices, opens mic or system-audio loopback, reports device loss. | yes |
| `system` | Existing capability probe, plus: sleep-prevention guard, own-window capture exclusion, session lock/unlock events, free disk space. | yes |
| `recording` | The recording session: clock, pause accounting, state machine, video pacer, drift correction, FFmpeg process manager, meeting folder layout, `meeting.json`, disk guard, recovery. | no |
| `storage` | Migration 0002 (presets), meeting rows, recovery queries. | no |
| `commands` | Start, pause, resume, stop, cancel, list devices and monitors, preview frame, recording state; events to the UI (state, levels, warnings). | no |

### One clock, pause cut out

Every frame and audio chunk is stamped against one monotonic clock when it is captured. Media time is
`now - start - total paused time`. Anything that arrives while paused is dropped, so paused time never exists in any
file and every later transcript timestamp equals playback position (SPEC Section 5).
**Pause stops all capture devices** (screen, mic, system audio) and resume reopens them. Paused therefore means "not
capturing", visibly (no capture border, no open microphone). The cost is a short restart on resume, which does not
matter because paused time is cut.

### Staying in sync for two hours

Audio devices run on their own clocks, which differ from the monotonic clock by tens of parts per million. Over two
hours 20 ppm is about 144 ms, more than the 100 ms budget. So each audio stream is checked against the clock as it
arrives, and the device rate is measured against media time; only if it is off by more than 3 ppm is a small correction (linear interpolation, no extra dependency) applied, so a device with good time is written bit-exact. A gap in
delivery is filled with silence and logged. Video is paced at constant frame rate against the same clock: frame *k* is
emitted for media time *k / fps*, repeating the latest capture when the screen is static (Windows.Graphics.Capture only
delivers a frame when something changes) and never letting video run shorter or longer than media time.

### Encoding: one FFmpeg process per stream, one mux at the end

| Process | Input (pipe) | Output while recording |
|---|---|---|
| video | raw BGRA frames at the chosen size and fps | fragmented MP4 (`h264_mf`) |
| mic | float PCM, 48 kHz | `microphone.flac` |
| system audio | float PCM, 48 kHz | `system-audio.flac` |

On stop, one more FFmpeg run copies the video, mixes the two FLAC tracks (each starts at media time 0 and carries its own leading silence)
into AAC, and writes `recording.mp4` (or `recording.m4a` for audio-only). Temporary files are deleted only after that
succeeds.

Why this shape: each track is independently crash-safe (FLAC frames are self-describing; a fragmented MP4 is playable
without its final index); a failed encoder cannot stop the other tracks (SPEC "Isolation"); the mix is built from the
lossless sources; and the recorder never needs to understand FFmpeg's internals. The price is a finishing step after
stop. For a 2-hour meeting this is expected to take seconds to about a minute and will be measured and shown as
progress.

The FFmpeg binary is a Tauri sidecar, started with `std::process::Command` (no shell plugin needed). On first use the
app runs `ffmpeg -version` and **refuses to run a build whose configuration contains `--enable-gpl` or
`--enable-nonfree`**, so a GPL build can never ship by accident. A script downloads the pinned release, checks its
SHA-256, and extracts only `ffmpeg.exe` into `src-tauri/binaries/` (git-ignored). The sidecar is configured in a
Windows-only config file so the macOS CI build does not need a macOS FFmpeg yet.

H.264 choice, as settled by M0: `h264_mf` with `-hw_encoding 1` and NV12 input first (on this machine it picks the
Intel Quick Sync encoder at about 0.11 CPU-s per video-s; without `-hw_encoding` it silently picks a software encoder at
0.42), then `libopenh264` (0.12 to 0.19), then `h264_mf` in software mode, otherwise the UI offers audio-only. The
detected encoder fills the existing `video_encoders` capability. See decision 2.

### Settings and constraints found in M0

These come from measurements (details in [phase-2-spikes.md](phase-2-spikes.md)) and are binding for M1 and M2.

- Fragmented MP4 must use `-movflags +frag_keyframe+empty_moov+default_base_moof+delay_moov`; every track uses
  `-flush_packets 1`. Without these a hard kill loses the whole file (hardware encoder) or up to a megabyte of FLAC.
- The capture frames are converted from BGRA to NV12 before encoding (about 0.11 CPU-s per video-s at 1080p15).
- System-audio loopback delivers nothing while no program plays audio. The recorder keeps a silent render stream open
  on the same output device while recording and still fills any remaining gap with silence by clock.
- Windows.Graphics.Capture sends the first frame within about 0.2 s and then only on change (gaps up to 9.55 s seen),
  so the pacer repeats the last frame. Monitors are chosen by device name or handle, never by index; sizes are physical
  pixels.
- Recovery cuts `recording.mp4` at the shortest common end and keeps the FLAC tracks at full length.
- cpal 0.18.2 (the released API, not GitHub master) is used for both microphone and loopback; overruns arrive as
  `ErrorKind::Xrun` through the error callback.
- Measured clock drift on this machine's built-in devices is about 0.2 ppm over 10 minutes, so the resampler is
  expected to be idle there. It stays in the plan for USB and Bluetooth devices and is measured with the external
  microphone in M4.

### Failure handling

| Event | Behaviour |
|---|---|
| Mic or headset unplugged | Keep recording the other streams. Warn in the UI. Fill the gap with silence so tracks stay aligned. Log the gap in `meeting.json`. Reopen the device if it returns. |
| Monitor disconnected | Keep recording audio. Repeat black frames for the gap. Warn and log. |
| Screen locked | Keep recording audio, black frames for the gap, mark it (session lock events from Windows). |
| Disk space | Warn at 2 GB free, stop cleanly at 500 MB free, keep everything recorded so far. Checked every 10 s. (Defaults, see decision 5.) |
| Encoder process dies | Record the error, keep the other streams running, show a warning. |
| App killed or crashes | Meeting row stays `recording`. Next launch offers Recover or Discard. Recovery rewrites the FLAC headers, remuxes the video, runs the same final mux, and marks the meeting `recovered` in `meeting.json`. |
| Quit or close window while recording | Close hides to the tray. Quit asks: stop and save, or keep recording. |

Sleep prevention uses a Windows power request (system required; display required too in screen mode, so the display
does not turn off and stop frames). It cannot stop a user-initiated sleep (lid closed). A long jump in the clock is
logged as a gap.

Own windows are excluded with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the main and control windows,
**only while recording**, so the user can still take screenshots of the app the rest of the time.

## 4. UI

- **Record screen:** mode, monitor picker with live preview (a low-rate preview image from the same capture API),
  microphone picker or off, system audio on/off, quality and fps presets (default 1080p / 15 fps), title, storage
  folder (from Settings), estimates (disk per hour, free space), named presets saved in SQLite (migration 0002).
- **Control window:** small, always on top, no title bar. Shows a recording indicator, elapsed time, mic and system
  level meters, Pause and Stop. Created when recording starts and closed at the end.
- **Tray icon:** idle / recording / paused states, menu with Start, Pause, Stop, Open, Quit.
- **Global hotkeys:** start/stop and pause/resume, configurable in Settings, with a clear message when a combination
  is already taken. Defaults are in Section 10.
- **Consent reminder:** a dialog before recording starts, controlled by the existing setting.
- **Recovery prompt:** at launch if an unfinished recording is found.
- **Playback:** after stopping, "Recording saved" with Play and Open folder. The Record screen lists the 10 most
  recent recordings with the same actions. The `<video>` element reads the file through Tauri's asset protocol,
  limited to the storage folder. The Library screen stays "Not yet implemented" until Phase 4.

## 5. Files and data

- Folder: `YYYY-MM-DD_HHMM_Title` in the storage folder, local time. Titles are sanitized: invalid characters removed,
  trailing dots and spaces trimmed, Windows reserved names (CON, PRN, AUX, NUL, COM1-9, LPT1-9, with or without an
  extension) avoided, length limited so the full path stays under the Windows limit. A name collision gets `_2`, `_3`,
  and an existing folder is never overwritten.
- Only files that were really produced are written. During recording the folder also holds temporary track files;
  they are removed after a successful finish and are the inputs for recovery.
- `meeting.json` v1, documented in `docs/`: app version, times, mode, devices (names and formats), encoder, pause
  intervals, gaps (stream, start, end, reason), errors, `recovered`.
- Migration 0002 adds `recording_presets`. `meetings.status` values used: `recording`, `finalizing`, `complete`,
  `failed`, `cancelled`.

## 6. New dependencies

All are license-checked and added to `LICENSES.md` with `pnpm licenses:generate`, and each must be maintained and
compatible with Tauri 2 before it is added.

Rust: `cpal`, `windows-capture`, `windows` (specific features only), `libc` (macOS local time), an image encoder for preview frames (cpal, windows-capture, windows, libc and sysinfo's disk feature are in since M1 and M2; the rest arrive in M3),
`tauri-plugin-global-shortcut`, `tauri-plugin-single-instance`, `tauri-plugin-opener`, Tauri features for the tray icon,
and `sysinfo`'s disk feature. Single-instance is added so two copies of the app cannot both record or fight over the
database. Frontend: none expected.

## 7. Tests

**Automated, run in CI, no hardware needed.** Test doubles (a synthetic screen and synthetic audio sources that emit
known patterns) are labelled as such and live only in tests.

- Clock and pause arithmetic; the video pacer (bursts, stalls, static screen); drift correction against a simulated
  20 ppm clock error over a simulated two hours (residual must stay under 20 ms); the recording state machine,
  including illegal transitions.
- Folder naming: reserved names, long and non-Latin titles, collisions, trailing dots.
- `meeting.json` round trip; gap logging; disk guard with a fake disk.
- End to end with the real FFmpeg and synthetic sources: a 20 s recording with a pause. Checks that duration equals
  active time, audio and video lengths agree within one frame, both FLACs decode, the MP4 decodes.
- Kill test: a child process records, is killed mid-way, recovery runs, output decodes.
- Vitest for the setup screen, controls, consent dialog, warnings and recovery prompt.
- The log guard keeps running over the new code.

**On this machine (real devices).** Results are reported with raw numbers and the machine named.

| Acceptance criterion | Method | Who |
|---|---|---|
| 2-hour recording: drift under 100 ms, memory flat | The test tool emits a beep and a screen flash together every 60 s. After the run, offsets between marker pairs at the start and end are measured from the files. Working-set memory is sampled every minute; "flat" is defined as under 5% growth between minute 10 and minute 120. Needs the machine left alone for 2+ hours. | I run it; you leave the machine idle |
| Killing the app mid-recording leaves a recoverable, playable file | I kill the process mid-recording, relaunch, recover, decode the result. | I run it |
| Unplugging the mic does not stop the recording | Needs an external USB or 3.5 mm microphone or headset (this machine has none connected). | You plug in and unplug on request |
| Control window not in the recorded video | The test build paints the control window a known solid color; frames are extracted from the recording and scanned for it. | I run it |
| Also checked | A 1-minute screen + audio recording on each of the 3 monitors; which encoder was used; CPU use while recording. | I run it |

## 8. Order of work

Each step ends with passing tests and a commit. I stop and report if a step breaks a plan assumption.

| Step | Contents | Gate |
|---|---|---|
| M0 Spikes (**done 2026-10-16**) | FFmpeg download, license and encoder check; a few seconds of real capture; mic and loopback timestamps and device formats; a 10-minute drift measurement. Findings in `docs/phase-2-spikes.md`. | No assumption failed. One refinement: encoder order (see Section 3). |
| M1 Core (**done 2026-10-16**) | Everything that needs no hardware: clock, state machine, pacer, drift correction, folder layout, `meeting.json`, migration 0002, disk guard, FFmpeg manager, synthetic sources, end-to-end and recovery tests. | Tests green locally (174 Rust tests, FFmpeg required) and in CI. |
| M2 Windows sources (**done 2026-10-16**) | Real capture and audio, sleep guard, window exclusion, session events, disk space, capability probe updates. | Smoke recordings on this machine: passed (see "What M2 settled"). |
| M3 UI | Record screen, control window, tray, hotkeys, consent, banners, recovery prompt, playback. | Vitest plus a manual pass. |
| M4 Acceptance | The four criteria above, docs, `LICENSES.md`, CI changes (fetch and cache the sidecar, run FFmpeg tests on the Windows runner). | Phase report. |

CI note: the Windows runner downloads the pinned FFmpeg (`pnpm fetch:ffmpeg`, checksum-verified) and runs the tests with
`REQUIRE_FFMPEG=1`, so a missing FFmpeg fails the build instead of skipping tests. The macOS runner has no FFmpeg yet;
those tests skip there and it only has to compile.

### What M1 settled (design changes made while building it)

- **Two-phase sources.** A source has `prepare()` (slow: open the device) and `start()` (fast: begin delivering). The
  session prepares every source and launches every encoder first, then starts the recording clock, then starts the
  sources back to back. Before this, launching FFmpeg for the second and third stream was counted as 340 ms of lost
  audio at the start.
- **Events carry the source's own timestamp.** "Screen locked" and "device unplugged" are placed on the timeline at
  the time the source saw them, and the pacer applies gaps in time order, so a busy worker cannot move a gap.
- **One outage is one `meeting.json` entry.** Gap pieces that touch are merged.
- **Start-up delay is `startOffsetMs`, not a gap**, unless it is a second or more.
- **A slow encoder is not a dead one.** Workers wait up to 30 s for a running encoder to accept data (10 s while
  stopping) before declaring it failed. Found when five hardware-encoder sessions started at once: the hardware encoder
  can take seconds to start, and a 5 s limit made the video disappear.
- **Recovery keeps the FLAC tracks at full length.** Only `recording.mp4` is cut at the shortest common end; real
  recorded audio is never discarded, so after a recovery the FLAC files can be a second or two longer.
- **Deleting a folder retries for up to 5 s**, because Windows keeps files locked briefly after a process dies and while
  antivirus scans a new file.
- **Test doubles are never in the app.** `recording::testing` and the `synthetic-recorder` test program exist only with
  the `test-doubles` feature, which a self dev-dependency turns on for `cargo test`.

### What M2 settled

Real devices on IV-LT-Han (Windows 11 build 26200, Intel Quick Sync encoder, 3 monitors, built-in microphone and
speakers). The seven hardware smoke tests (`tests/hardware_smoke.rs`, run with `--ignored`) pass, also all together in
one process.

- **Verified with a program that checks the output, not by looking:** a real 8 s recording of the primary monitor,
  microphone and system audio. All three files have the recording's length (within 0.3 s). The video has the monitor's
  physical size and shows a known magenta window (pixel check). A -30 dBFS tone played on the speakers is in the
  system-audio track (-27.9 dB measured). The hardware encoder was used. No gaps, no errors.
- **Hiding the app's own windows works.** A test window is in the video until `SetWindowDisplayAffinity(
  WDA_EXCLUDEFROMCAPTURE)` is applied, and gone after: pixel [254, 0, 253] before, [37, 37, 37] (the desktop) after.
  This is the primitive behind the acceptance criterion "the control window is not in the video"; the criterion itself
  is checked in M4 with the real control window.
- **Pause with real devices** cuts the paused time from all three tracks (3.5 s paused, tracks equal to the media
  length). Restarting a device after a pause can take up to about 0.3 s, during which that track is silent; when it is
  250 ms or more it is listed in `meeting.json` as a gap.
- **Parallel source start.** The screen capture takes about 0.8 s to start. Starting sources one after another made
  the audio begin 840 ms late; starting them together brings it to 41 to 49 ms.
- **Audio after the stop is cut off.** Devices deliver for a moment after stop is requested; that audio is past the end
  of the recording and is dropped, so every track ends where the media ends.
- **A crash found only on real hardware, and fixed.** The second recording in one run of the program crashed with an
  access violation inside `windows-capture`: it caches a Windows Runtime factory, and the apartment that factory lives
  in is destroyed when the last thread using it ends. The fix keeps the apartment open for the life of the process
  (`system::keep_component_runtime_alive`, called before the first device is used). Regression tests record several
  times in one process.
- **Keep-awake** uses a Windows power request (process-wide, not tied to a thread); the display is kept on only while
  recording the screen. Tested with the real API and, in whole recordings, with a test double.
- **Capabilities** now report system-audio capture (the default output device) and capture exclusion (Windows build
  19041 or newer) from the machine. Video encoders are detected by the recording code and merged in M3.

**Not verified on real hardware yet** (implemented, but never exercised): the workstation being locked and unlocked
(the message plumbing is tested with simulated messages only; locking the real session would lock the user out),
a monitor being unplugged and coming back, a microphone being unplugged (needs the external microphone in M4; the
synthetic version is tested), the default output device changing during a recording (a device whose format changed is
not reopened), and whether `WithoutBorder` really removes the border Windows draws around a captured monitor.

Measured or simulated in M1: two simulated hours with a device clock 20 ppm fast or slow stay within 20 ms of media
time (simulation, not real hardware). A process killed hard mid-recording (with its FFmpeg children) is recovered into
one playable file in 5 of 5 repeated runs.

## 9. Risks

1. **H.264 licensing.** Encoding H.264 for commercial distribution can carry patent-licensing obligations. Using the
   operating system's encoders (`h264_mf`) leaves that with the OS vendor; OpenH264 compiled from source in a third-party
   FFmpeg build does not carry Cisco's patent grant (Cisco covers only its own prebuilt binaries). Needs legal advice
   before distribution. Not a blocker for development.
2. **Installer size.** The pinned BtbN LGPL build is large (164-170 MB zipped, all libraries included). Fine for development.
   A custom minimal LGPL build (only the encoders and muxers used) in CI is the likely answer before distribution.
3. **LGPL duties.** Shipping FFmpeg means shipping its license text and a way to get the exact source. Tracked in
   `LICENSES.md` and the installer resources.
4. **Allow-listing.** `ffmpeg.exe` is a downloaded executable. During development it lives under `C:\projects`, which
   IT allow-listed. A distributed build needs the code-signing certificate from SPEC Section 12.
5. **Capture border.** Windows may draw a border around the captured monitor. It is not captured in the video, but it is
   visible. `WithoutBorder` is requested and starts without error on build 26200; whether the border is really gone was not observed.
6. **Still unverified after M0:** whether the adaptive resampler holds the 100 ms budget on a drifting (USB or
   Bluetooth) device; whether the capture border is really hidden by `WithoutBorder`; whether the pinned FFmpeg release
   stays downloadable (dated BtbN releases expire, so a copy we control is needed); behaviour on the reference-class
   machine, which is not available here.

## 10. Decisions (2026-10-16)

The plan was approved. The recommendations below were accepted, except the hotkeys, which were changed.

| # | Decision | Outcome |
|---|---|---|
| 1 | Overall plan, one FFmpeg process per stream plus a final mux. | Approved. |
| 2 | `h264_mf` first (hardware, else Microsoft software encoder), OpenH264 only as a second fallback. | Accepted. |
| 3 | Pinned BtbN LGPL FFmpeg now, custom minimal build later. | Accepted. |
| 4 | Hardware for the unplug test, 2+ hours of idle machine time for the soak test. | Accepted. To be requested when needed. |
| 5 | Defaults. | **Hotkeys: Ctrl+Shift+S start/stop, Ctrl+Shift+P pause/resume** (changed from F9/F10). Disk warn 2 GB, stop 500 MB; display kept on during screen recording; own windows hidden from capture only while recording; black frames during lock or monitor loss; pause stops all devices. All accepted. |
| 6 | Playback and the 10 most recent recordings on the Record screen; Library stays Phase 4. | Accepted. |
| 7 | `tauri-plugin-single-instance`. | Accepted. |

Hotkey caveat. A global shortcut takes its key combination away from every other program while MeetingMind is
running. Ctrl+Shift+S is "Save As" in many programs and Ctrl+Shift+P opens the command palette in VS Code; pressing
them there would start a recording or pause one. The app cannot detect this kind of clash (registration succeeds), so
the combinations are configurable in Settings, and the recording indicator makes an accidental start visible.
