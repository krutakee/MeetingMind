# SPEC.md — MeetingMind (Recorder + Local Transcription)

> Reference specification. Standing working rules live in CLAUDE.md.
> Fill in every `[TODO: ...]` before starting Phase 1.

## 1. Product summary

MeetingMind is a desktop app that records meetings (screen video, microphone, system audio) and transcribes speech live, in the language being spoken, using a local Whisper model. When recording stops, it saves the recording and the final transcript together in one meeting folder.

It has **no running costs**: no cloud services, no API keys, no accounts. Meeting content never leaves the machine.

Working name: MeetingMind (keep the product name configurable).

## 2. Scope

### In scope
- Windows (MVP), then macOS.
- Full-screen recording, or audio-only recording.
- Microphone and system audio, recorded as separate tracks.
- Live transcription in the meeting's own language, with a local Whisper model (whisper.cpp).
- A more accurate final transcription pass after the meeting.
- Saving and exporting the recording and transcript.
- A library of past meetings: search, reopen, play back, edit the transcript.

### Out of scope (this version)
- AI summaries, decisions, action items.
- Live translation into another language.
- Cloud providers, API keys, accounts, cloud sync.
- Window or region capture.
- Voice diarization (beyond the You/Others split).
- Linux.

### Future-proofing
Keep `transcript.json` stable, versioned and documented. Future modules (AI summaries, translation) must be able to read it without any change to recording or transcription code.

## 3. Platforms

| Platform | Status | Notes |
|---|---|---|
| Windows 11 | MVP | Primary dev machine |
| macOS 13+ | Phase after MVP | Needs a real Mac for testing and an Apple Developer account for signing and notarization |
| Linux | Out of scope | |

All OS-specific code lives inside the capture, audio and system-integration modules, behind traits. No OS-specific APIs in transcription, storage, library or UI code. CI builds on `windows-latest` and `macos-latest` from Phase 1.

## 4. Technology

- **Desktop:** Tauri 2, Rust backend.
- **Frontend:** React, TypeScript, Tailwind, an accessible component library. Light and dark themes.
- **Screen capture:**
  - Windows: Windows.Graphics.Capture (e.g. the `windows-capture` crate).
  - macOS: ScreenCaptureKit.
- **Audio:**
  - Windows: microphone via `cpal`; system audio via WASAPI loopback.
  - macOS: microphone via `cpal`; system audio via ScreenCaptureKit audio.
- **Echo cancellation:** WebRTC audio processing (e.g. a `webrtc-audio-processing` binding) on the mic stream, using system audio as the reference signal. See Section 6.
- **Encoding:** FFmpeg as a sidecar process, LGPL build only (no GPL encoders such as x264). Prefer hardware H.264 encoders (Media Foundation, VideoToolbox) and fall back to OpenH264.
- **Transcription:** whisper.cpp via `whisper-rs`, with Silero VAD.
  - Windows GPU: Vulkan backend by default. It works across NVIDIA, AMD and Intel GPUs without bundling the CUDA runtime. CUDA is optional.
  - macOS GPU: Metal.
  - CPU fallback everywhere.
- **Storage:** SQLite (`sqlx` or `rusqlite`) with versioned migrations and FTS5 for transcript search.
- **Testing:** Rust unit and integration tests, Vitest for the frontend. Sample audio must come from openly licensed sources (e.g. LibriSpeech, CC BY 4.0) or be recorded for the project; record the source.

Verify each dependency is maintained, compatible with Tauri 2, and licensed for commercial distribution. Record licenses in `LICENSES.md`.

## 5. Recording

### Modes
- **Screen + audio:** the selected monitor plus the selected audio sources.
- **Audio only:** no video. Much smaller files and lower CPU use.

### Setup screen (savable as presets)
- **Mode:** screen + audio, or audio only.
- **Monitor:** which monitor to record, with a live preview.
- **Microphone:** device picker, or off.
- **System audio:** on or off.
- **Video:** quality and frame rate presets (e.g. 1080p/15 fps as the default; meetings rarely need 30 fps).
- **Transcription:** language mode and Whisper model (Section 6).
- **Meeting title.**
- **Storage folder:** where meetings are saved.
- **Estimates:** expected disk use per hour, and free space on the target drive.

### Controls
- Start, pause, resume, stop, and cancel (with confirmation).
- Elapsed time, a visible recording indicator, and live level meters for the mic and system audio.
- A compact, always-on-top control window.
- Global hotkeys for start/stop and pause/resume (configurable).
- A tray icon with recording status.

### Requirements
- **One clock.** Timestamp every stream (video, mic, system audio) against a single monotonic clock at capture time.
- **Pause semantics.** Paused time is cut from the recording. All transcript timestamps use the final media timeline, so they always match playback position. Transcription pauses with recording.
- **Separate tracks.** Mic and system audio are saved separately (lossless FLAC) and also mixed into the MP4.
- **Crash safety.** Write fragmented MP4 or segment files during recording; remux to a normal MP4 on stop. After a crash, the recording up to that point must be recoverable on next launch.
- **Isolation.** Recording continues even if transcription fails or falls behind.
- **Hide own windows.** Exclude MeetingMind's own windows from the capture:
  - Windows: `SetWindowDisplayAffinity` with `WDA_EXCLUDEFROMCAPTURE`.
  - macOS: a ScreenCaptureKit content filter.
- **Stay awake.** Prevent system sleep while recording. If the screen locks, keep recording audio and mark the video gap.
- **Device changes.** If a mic or headset is unplugged, or a monitor disconnects, keep recording whatever is still available. Show a clear warning and log the gap in `meeting.json`. Never stop the whole recording silently.
- **Disk space.** Warn at a low threshold. At a critical threshold, stop recording cleanly and keep everything recorded so far.
- **Permissions.** Detect missing permissions (macOS screen recording and microphone) and show step-by-step instructions.
- **Consent reminder.** Show a reminder about getting participant consent before recording starts. The user can turn it off.

### Capability detection
A capability layer reports what works on this machine:
- system audio capture
- capture exclusion
- GPU backend and VRAM
- available video encoders
- RAM and CPU cores

If something is unavailable, the UI says why and offers an alternative. Never fake a capability.

## 6. Live transcription

### Pipeline
1. Copy audio from each stream into a bounded queue (backpressure, no unbounded memory growth). Resample to 16 kHz mono for Whisper only; the recording itself keeps full quality.
2. Run echo cancellation on the mic stream (below).
3. Use VAD to split speech into utterances and skip silence.
4. Transcribe utterances with whisper.cpp, using overlapping context so no words are lost at boundaries.
5. Show provisional text (visually distinct, e.g. greyed), then replace it with finalized text. Never duplicate words.
6. Persist finalized segments to SQLite as they arrive.

### Speaker labels
- Mic → **"You"**.
- System audio → **"Others"**.
- The user can rename the labels per meeting.

### Echo and double transcription
Without headphones, the mic also picks up the other participants from the speakers, so their speech would be transcribed twice: once as "Others" and once as "You". To prevent this:
- Apply echo cancellation to the mic stream, using system audio as the reference.
- As a safety net, when a mic segment overlaps in time with a system-audio segment and the text is highly similar, mark the mic segment as a duplicate and hide it (recoverable, not deleted).
- Show a tip recommending headphones when echo is detected.

### Hallucination control
Whisper can invent text on silence or noise (e.g. "Thank you for watching") and can get stuck repeating a phrase. To prevent this:
- Only send VAD-detected speech to Whisper.
- Discard segments with a high no-speech probability or a very low average log-probability (configurable thresholds).
- Detect repetition loops and cut them off.
- Keep a configurable filter list of known hallucination phrases, applied only to very short, isolated segments.
- Keep discarded segments in a debug log (without text in normal logs) so the thresholds can be tuned.

### Language modes
The user picks one mode in the setup screen; it is saved with presets.

**Single language (default).** Whisper is forced to the chosen language. This is the most accurate option and is recommended when the meeting language is known.

**Auto-detect.** Whisper detects the language. To avoid misdetection at the start of a meeting:
- Lock the language per stream once detection is confident across the first several utterances.
- Let the user change it at any time from the transcript panel; changing it re-transcribes from the next utterance.

**Two languages (advanced).** The user picks two languages. For each utterance:
1. Run language detection.
2. Compare only the probabilities of the two selected languages and ignore all others.
3. Transcribe the utterance in the winner.

Two-language mode also uses hysteresis: a stream keeps its current language unless the other selected language is clearly more probable (configurable threshold).

**Rules for all modes:**
- The mic and system audio streams detect languages independently.
- Every segment stores its language. The transcript shows a small language tag only when more than one language appears in the meeting.
- Auto-detect and two-language mode require a multilingual model (never an `.en` model).
- **Known limitation:** mixing two languages inside one sentence is not handled reliably. State this in the help text.

### Transcript display
- Show the transcript only in the spoken language. No translation in this version.
- Two-panel layout: preview and controls on the left, live transcript on the right (resizable, collapsible).
- Each segment shows its speaker label and timestamp.
- Auto-scroll, a pause-auto-scroll option, and a jump-to-latest button.
- Search within the transcript during recording.
- A copy-transcript button.
- Status indicators: model, language mode, transcription lag, GPU/CPU, errors.

### Final pass (optional setting, default on)
When recording stops, re-transcribe the full audio with a more accurate model and replace the live draft.
- Runs in the background with progress, an estimated time, and cancel. The rest of the app stays usable.
- If the final pass fails or is cancelled, keep the live draft.
- The final pass never overwrites user edits (Section 7).
- The user can re-run it later with a different model or language mode.

### Models
- The app lists Whisper models with download size, speed and accuracy, and recommends a live model and a final-pass model based on the detected hardware.
- Downloads are user-initiated, show progress, can resume, and are verified against SHA-256 checksums.
- Manual import of a model file is supported, for machines where downloads are blocked by a firewall.
- Models are stored in the app data folder and can be deleted from settings.
- A model download is the only network activity in normal use. It never includes meeting content.

## 7. Saving and export

Each meeting gets its own folder:

```
MeetingMind/Meetings/YYYY-MM-DD_HHMM_Meeting-Title/
  recording.mp4          video + mixed audio (screen mode)
  recording.m4a          mixed audio (audio-only mode)
  microphone.flac        if recorded
  system-audio.flac      if recorded
  transcript.json        canonical, versioned schema
  transcript.txt         "[HH:MM:SS] Speaker: text"
  transcript.md
  transcript.srt
  transcript.vtt
  meeting.json           metadata: times, devices, models, settings, gaps, errors
```

### Rules
- Only write files that were actually produced. No empty placeholder files.
- Sanitize meeting titles for file names (invalid characters, length limits, Windows reserved names).
- Never overwrite an existing meeting folder. Resolve name collisions.
- **Edits are preserved.** `transcript.json` keeps the original text and the user-edited text separately. Exports use edited text. Re-running the final pass asks before replacing any edited segment.
- SRT and VTT must be valid and correctly timed, with sensible line lengths.
- Exports can be regenerated from the meeting screen into a user-chosen folder, optionally as a ZIP of selected files.

### Storage settings
- Optionally delete the separate FLAC tracks after the final pass succeeds (saves space).
- Show the disk usage per meeting and in total.

## 8. Meeting library

- List of meetings with title, date, duration and mode; sort and filter by date.
- Full-text search across all transcripts (FTS5). Results jump to the matching segment.
- **Reopen a meeting:** a player with the transcript beside it.
  - Clicking a segment seeks the player.
  - The current segment is highlighted during playback.
  - Playback speed control.
- **Editing:**
  - Edit transcript text, with timestamps fixed.
  - Rename speakers.
  - Correct a segment's language and re-transcribe it.
- Rename and delete meetings. Deleting needs a confirmation and removes the folder, database rows and derived files.
- **Recovery on startup:** detect unfinished recordings and interrupted final passes, and offer to recover, resume or discard.

## 9. Database (minimum)

- **meetings:** id, title, start/end time, duration, mode, folder path, status, capture settings, language mode, models used, created/updated timestamps.
- **segments:** id, meeting id, stream (mic/system), speaker label, start/end time (media timeline), original text, edited text, language, final flag, duplicate flag, pass (live/final).
- **speakers:** meeting id, label, display name.
- **jobs:** id, meeting id, type (final pass, export, recovery, model download), status, progress, error, retry count, timestamps.
- **models:** name, file path, size, checksum, installed date.
- **settings:** app preferences.

Use foreign keys, indexes and transactions. Never load all transcripts into memory at once.

## 10. Privacy and security

- Meeting content never leaves the machine. There is no telemetry. The only network use is user-initiated model downloads.
- The recording indicator is always visible while recording.
- Logs never contain transcript text.
- Meeting files are not encrypted by the app. Document that users should rely on disk encryption (BitLocker / FileVault) and choose the storage folder accordingly.

## 11. Phases and acceptance criteria

Complete one phase at a time. At the end of each phase: run tests, commit, then stop and report what works, what doesn't, and what's blocked.

All performance criteria are measured on the reference machine, a typical mid-range business laptop:

- **CPU:** Intel Core i5 / Ultra 5 or AMD Ryzen 5, from 2022 or newer (8+ threads).
- **RAM:** 16 GB.
- **GPU:** integrated graphics only (e.g. Intel Iris Xe or AMD Radeon 680M/780M), no dedicated GPU.
- **Storage:** SSD.

If the app meets the criteria here, it will run well on most office PCs. Machines with a dedicated GPU should simply be faster. Default model choices on this class of machine:

- **Live transcription:** Whisper `small` (multilingual).
- **Final pass:** `large-v3-turbo`. It may run slower than real time on this machine, which is acceptable because it runs in the background.

Each phase report must state which machine the tests actually ran on.

### Phase 1 — App shell
Tauri + React app, navigation, settings, SQLite with migrations, logging, capability detection skeleton, CI on Windows and macOS.

Done when:
- The app launches on Windows.
- CI is green on both platforms.
- Migration tests pass.

### Phase 2 — Windows recording
Screen + audio and audio-only modes, pause/resume/stop, crash-safe files, own-window exclusion, sleep prevention, device-change handling, playback.

Done when:
- Over a 2-hour recording, audio/video drift stays under 100 ms and memory use stays flat.
- Killing the app mid-recording leaves a recoverable, playable file.
- Unplugging the mic mid-recording does not stop the recording.
- The control window does not appear in the recorded video.

### Phase 3 — Live transcription
whisper.cpp with VAD, live panel, You/Others labels, echo handling, hallucination filters, language modes, model download and recommendation, segment persistence.

Done when:
- Transcript lag (end of speech → finalized text on screen) stays under 5 seconds.
- There are no duplicated words at boundaries in the sample audio tests.
- No hallucinated text appears in a 10-minute silence/noise test.
- In a speakers-without-headphones test, duplicate "You" segments are suppressed.
- Recording continues if transcription is forcibly failed.
- Accuracy is reported separately for each test language.

### Phase 4 — Save, export, library (MVP complete)
Final pass, all export formats, library, search, synced playback, transcript editing, recovery, storage settings.

Done when:
- SRT/VTT validate.
- The final pass never overwrites user edits without asking.
- Search works across 50+ test meetings.
- A meeting reopens with click-to-seek working.
- Recovery works after a simulated crash during the final pass.

### Phase 5 — macOS
ScreenCaptureKit capture and audio, permissions flow, Metal acceleration, capture exclusion, signing and notarization.

Done when: the Phase 2–4 acceptance criteria pass on a real Mac.

### Later (not planned yet)
Window/region capture, voice diarization, live translation, optional AI summary module, auto-updates.

## 12. Distribution

- **Windows:** a code-signing certificate is needed before distributing to other machines. Signed builds allow publisher-based allowlisting (e.g. ThreatLocker) instead of per-hash approvals for the app and its sidecar binaries.
- **macOS:** Apple Developer account, code signing, notarization.
- **Installers:** MSI or NSIS on Windows, DMG on macOS.

## 13. Instructions for Claude Code

1. Read this file. This is a new, empty repository.
2. An earlier prototype built from a larger spec may exist at [TODO: path to old project folder]. Treat it as read-only reference: you may reuse code from it that fits this spec, but never modify that folder.
3. Create or update CLAUDE.md (max ~80 lines) with: a project summary, build/test commands, the current phase and its acceptance criteria, and the working rules below.
4. Update ROADMAP.md and TODO.md to match Section 11.
5. Show me the plan and stop. Wait for approval before writing code.

### Working rules (copy into CLAUDE.md)
- One phase at a time. Test, commit, report, stop.
- Never fabricate test results, recording success, or platform capabilities.
- Never replace working functionality with mock data. Use mocks only in tests, and label them clearly.
- OS-specific code only inside the capture, audio and system-integration modules.
- The dev machine uses ThreatLocker application allowlisting. If a command or executable is blocked, stop and report the exact executable and path. Never retry repeatedly or try to work around it.
