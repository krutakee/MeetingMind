# CLAUDE.md — MeetingMind

Standing instructions for Claude Code. The full reference is [SPEC.md](SPEC.md); plans live in
[ROADMAP.md](ROADMAP.md) and [TODO.md](TODO.md). If this file and SPEC.md disagree, SPEC.md wins — then fix this file.

## Project summary
MeetingMind is a desktop app (Tauri 2 + Rust backend, React/TypeScript/Tailwind frontend) that records
meetings (screen video, microphone, system audio) and transcribes speech live with a local Whisper model
(whisper.cpp via `whisper-rs`, Silero VAD). On stop it saves the recording and transcript in one meeting folder.
Windows 11 is the MVP; macOS follows. No cloud, no API keys, no accounts, no telemetry, no running costs.
The only network activity is user-initiated Whisper model downloads (SHA-256 verified, never meeting content).

Key constraints (details in SPEC.md):
- Mic and system audio are separate lossless FLAC tracks, also mixed into the MP4. One monotonic clock for all streams.
- Recording must survive transcription failure, device changes and crashes (fragmented MP4, recover on next launch).
- `transcript.json` is the canonical, versioned, documented schema; user edits are stored apart from original text.
- FFmpeg sidecar, LGPL build only (no GPL encoders such as x264). Record every dependency license in `LICENSES.md`.
- Storage: SQLite with versioned migrations + FTS5. Logs must never contain transcript text.

## Commands
Verified on the dev machine on 2026-10-16 (Windows 11, Node 22, pnpm 10.33, Rust 1.99 MSVC, VS 2022 Build Tools, WebView2).
The frontend `dist/` folder (repo root) must exist before any cargo command: run `pnpm build` first.
**BLOCKED (2026-10-16):** `pnpm tauri build` fails extracting the NSIS tool into `%LOCALAPPDATA%\tauri\nsis-3.13\`
("Access is denied") — see TODO.md. Do not re-run it until the user says it is unblocked; CI builds the installer.

- Install JS deps: `pnpm install` (pnpm is the package manager; version pinned in `package.json`)
- Frontend build: `pnpm build` (typecheck + Vite into `dist/`)
- Run app in dev: `pnpm tauri dev` (set `MEETINGMIND_LOG=meetingmind_lib=debug,info` for command-level logs)
- Build installer: `pnpm tauri build` (release exe compiles locally; the NSIS bundling step is blocked here)
- Frontend tests: `pnpm test` (Vitest)
- Frontend typecheck/lint: `pnpm typecheck`, `pnpm lint`
- Hardware smoke tests (real monitor, microphone, speakers; a magenta window and a quiet tone appear): from
  `src-tauri/`, `cargo test --test hardware_smoke -- --ignored --nocapture --test-threads=1`
- Install the pinned FFmpeg sidecar: `pnpm fetch:ffmpeg` (checksum-verified, Windows only for now). Needed before ANY cargo
  command on Windows: `tauri.windows.conf.json` bundles it, and the build stops if the file is missing.
- Run the app by hand: `pnpm tauri dev` (starts Vite and the app). With `cargo run` alone the window stays empty unless
  `pnpm dev` (Vite, port 1420) is running. Close the app before `cargo test`: a running `meetingmind.exe` locks the file.
- Rust tests: `cargo test --workspace` (run from `src-tauri/`). The real-FFmpeg tests skip if FFmpeg is missing; set
  `REQUIRE_FFMPEG=1` to make that a failure (CI does, on Windows). Test doubles live behind the `test-doubles` feature,
  which `cargo test` turns on by itself; the app never contains them.
- Rust lint/format: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` (from `src-tauri/`)
- Regenerate `LICENSES.md` after any dependency change: `pnpm licenses:generate`

## Current phase: Phase 2 — Windows recording (plan approved 2026-10-16). Phase 1 is done
Phase 1 delivered: Tauri + React app, navigation, settings, SQLite with migrations, logging, capability-detection
skeleton, CI on Windows and macOS. Acceptance: app launches on Windows (checked locally); CI green on `windows-latest`
and `macos-latest` (run 38941862010, commit f1edde7); migration tests pass (locally and in CI).

Phase 2 status: M0 to M4 done (2026-10-17); report: docs/phase-2-report.md. Three of the four acceptance criteria are met
and measured. **Open: unplugging the microphone, on a real external microphone** (passes with a synthetic one;
`src-tauri/tests/mic_unplug.rs` is ready and needs the hardware and a person). Phase 2 is not accepted until that is done.
Plan approved 2026-10-16: docs/phase-2-plan.md. Scope and acceptance criteria are in
SPEC.md Section 11 and ROADMAP.md. Per phase: run tests, commit, report what works / what doesn't / what's blocked, STOP.

## Architecture rules
- OS-specific code lives only in the capture, audio and system-integration modules, behind traits.
  No OS-specific APIs in transcription, storage, library or UI code.
- Transcription, storage and library code must work unchanged on Windows and macOS.
- Future modules (summaries, translation) read `transcript.json` only; do not couple them to recording code.
- Only write files that were actually produced. No empty placeholder files in meeting folders.

## Reference machine for performance claims
Mid-range business laptop: Intel Core i5/Ultra 5 or Ryzen 5 (2022+, 8+ threads), 16 GB RAM, integrated GPU only, SSD.
Every phase report must state which machine the tests actually ran on.
Default models on this class: live = Whisper `small` (multilingual); final pass = `large-v3-turbo`.

## Working rules
- One phase at a time. Test, commit, report, stop.
- Never fabricate test results, recording success, or platform capabilities.
- Never replace working functionality with mock data. Use mocks only in tests, and label them clearly.
- OS-specific code only inside the capture, audio and system-integration modules.
- The dev machine uses ThreatLocker application allowlisting. If a command or executable is blocked, stop and report
  the exact executable and path. Never retry repeatedly or try to work around it.
- An earlier prototype may exist (path not yet provided; see TODO.md). It is read-only reference: reuse fitting code,
  never modify that folder.
- Do not write code for a phase until its plan has been approved.
