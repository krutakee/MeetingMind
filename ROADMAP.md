# ROADMAP — MeetingMind

Mirrors [SPEC.md](SPEC.md) Section 11. If they disagree, SPEC.md wins. One phase at a time: test, commit, report, stop.

Status key: `not started` · `plan awaiting approval` · `in progress` · `done`

**Reference machine** (all performance criteria): Intel Core i5 / Ultra 5 or AMD Ryzen 5 (2022+, 8+ threads), 16 GB RAM,
integrated graphics only, SSD. Each phase report states which machine the tests actually ran on.
Default models on this class: live = Whisper `small` (multilingual); final pass = `large-v3-turbo`.

| Phase | Goal | Status |
|---|---|---|
| 1 | App shell | done (2026-10-16) |
| 2 | Windows recording | plan awaiting approval ([docs/phase-2-plan.md](docs/phase-2-plan.md)) |
| 3 | Live transcription | not started |
| 4 | Save, export, library (MVP complete) | not started |
| 5 | macOS | not started |
| Later | Not planned yet | — |

## Phase 1 — App shell
Tauri + React app, navigation, settings, SQLite with migrations, logging, capability detection skeleton,
CI on Windows and macOS.

Done when:
- The app launches on Windows.
- CI is green on both platforms.
- Migration tests pass.

## Phase 2 — Windows recording
Screen + audio and audio-only modes, pause/resume/stop, crash-safe files, own-window exclusion, sleep prevention,
device-change handling, playback.

Done when:
- Over a 2-hour recording, audio/video drift stays under 100 ms and memory use stays flat.
- Killing the app mid-recording leaves a recoverable, playable file.
- Unplugging the mic mid-recording does not stop the recording.
- The control window does not appear in the recorded video.

## Phase 3 — Live transcription
whisper.cpp with VAD, live panel, You/Others labels, echo handling, hallucination filters, language modes,
model download and recommendation, segment persistence.

Done when:
- Transcript lag (end of speech → finalized text on screen) stays under 5 seconds.
- There are no duplicated words at boundaries in the sample audio tests.
- No hallucinated text appears in a 10-minute silence/noise test.
- In a speakers-without-headphones test, duplicate "You" segments are suppressed.
- Recording continues if transcription is forcibly failed.
- Accuracy is reported separately for each test language.

## Phase 4 — Save, export, library (MVP complete)
Final pass, all export formats, library, search, synced playback, transcript editing, recovery, storage settings.

Done when:
- SRT/VTT validate.
- The final pass never overwrites user edits without asking.
- Search works across 50+ test meetings.
- A meeting reopens with click-to-seek working.
- Recovery works after a simulated crash during the final pass.

## Phase 5 — macOS
ScreenCaptureKit capture and audio, permissions flow, Metal acceleration, capture exclusion, signing and notarization.

Done when: the Phase 2–4 acceptance criteria pass on a real Mac.
Needs: a real Mac for testing and an Apple Developer account.

## Later (not planned yet)
Window/region capture, voice diarization, live translation, optional AI summary module, auto-updates.

## Distribution (cross-cutting, SPEC.md Section 12)
- Windows: code-signing certificate needed before distributing to other machines (enables publisher-based allowlisting,
  e.g. ThreatLocker, for the app and its sidecar binaries). Installer: MSI or NSIS.
- macOS: Apple Developer account, signing, notarization. Installer: DMG.
