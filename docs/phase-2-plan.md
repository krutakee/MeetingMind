# Phase 2 plan — Windows recording

**Status: approved 2026-10-16. Work starts with M0 (spikes). Hotkeys changed in review: see Section 10.**
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
| Encoders in that build | Whether it has `h264_mf` (Media Foundation), `libopenh264`, `aac`, `flac`, and a build config with no `--enable-gpl` / `--enable-nonfree`. | **Not verified. First spike task.** |
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
arrives, and an adaptive resampler (`rubato`, MIT; to be license-checked) corrects the measured ratio. A gap in
delivery is filled with silence and logged. Video is paced at constant frame rate against the same clock: frame *k* is
emitted for media time *k / fps*, repeating the latest capture when the screen is static (Windows.Graphics.Capture only
delivers a frame when something changes) and never letting video run shorter or longer than media time.

### Encoding: one FFmpeg process per stream, one mux at the end

| Process | Input (pipe) | Output while recording |
|---|---|---|
| video | raw BGRA frames at the chosen size and fps | fragmented MP4 (`h264_mf`) |
| mic | float PCM, 48 kHz | `microphone.flac` |
| system audio | float PCM, 48 kHz | `system-audio.flac` |

On stop, one more FFmpeg run copies the video, mixes the two FLAC tracks (aligned by their recorded start offsets)
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

H.264 choice: `h264_mf` first (it uses a hardware encoder when the machine has one and Microsoft's software encoder
otherwise), `libopenh264` second if the build includes it, otherwise the UI offers audio-only. The detected encoder
fills the existing `video_encoders` capability. See decision 2.

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

Sleep prevention uses `SetThreadExecutionState` (system required; display required too in screen mode, so the display
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

Rust: `cpal`, `windows-capture`, `windows` (specific features only), `rubato`, an image encoder for preview frames,
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
| M0 Spikes | FFmpeg download, license and encoder check; a few seconds of real capture; mic and loopback timestamps and device formats; a 10-minute drift measurement. Findings written to `docs/`. | If an assumption fails (for example no `h264_mf`, or cpal timestamps unusable), stop and ask before continuing. |
| M1 Core | Everything that needs no hardware: clock, state machine, pacer, drift correction, folder layout, `meeting.json`, migration 0002, disk guard, FFmpeg manager, synthetic sources, end-to-end and recovery tests. | Tests green locally and in CI. |
| M2 Windows sources | Real capture and audio, sleep guard, window exclusion, session events, disk space, capability probe updates. | Smoke recordings on this machine. |
| M3 UI | Record screen, control window, tray, hotkeys, consent, banners, recovery prompt, playback. | Vitest plus a manual pass. |
| M4 Acceptance | The four criteria above, docs, `LICENSES.md`, CI changes (fetch and cache the sidecar, run FFmpeg tests on the Windows runner). | Phase report. |

CI note: the Windows runner needs the pinned FFmpeg downloaded and cached. The macOS runner skips FFmpeg-dependent
tests and only has to compile.

## 9. Risks

1. **H.264 licensing.** Encoding H.264 for commercial distribution can carry patent-licensing obligations. Using the
   operating system's encoders (`h264_mf`) leaves that with the OS vendor; OpenH264 compiled from source in a third-party
   FFmpeg build does not carry Cisco's patent grant (Cisco covers only its own prebuilt binaries). Needs legal advice
   before distribution. Not a blocker for development.
2. **Installer size.** The pinned BtbN LGPL build is large (164-170 MB zipped, all libraries included). Fine for development.
   A custom minimal LGPL build (only the encoders and muxers used) in CI is the likely answer before distribution.
3. **LGPL duties.** Shipping FFmpeg means shipping its license text and a way to get the exact source. Tracked in
   `LICENSES.md` and the installer resources.
4. **Allow-listing.** `ffmpeg.exe` is a downloaded executable. During development it lives under `C:\projects\`, which
   IT allow-listed. A distributed build needs the code-signing certificate from SPEC Section 12.
5. **Capture border.** Windows may draw a border around the captured monitor. It is not captured in the video, but it is
   visible. Whether it can be turned off depends on the Windows build; to be checked in the spike and documented.
6. **Unverified assumptions** (M0 exists to settle them): FFmpeg encoder list, cpal timestamp quality, whether the
   adaptive resampler holds the 100 ms budget, behaviour of `h264_mf` with this machine's Intel graphics.

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
