# Phase 2 report — Windows recording

Written 2026-10-17. Plan: [phase-2-plan.md](phase-2-plan.md). Phase 2 is **not accepted yet**: three of the four
criteria are met and measured; the fourth (unplugging the microphone) needs an external microphone that this
computer does not have, so it has not been checked on real hardware (Section 7).

## 1. The four criteria

| Criterion | Result | Evidence |
|---|---|---|
| 2-hour recording: drift under 100 ms, memory flat | **Met**, twice | Two 2-hour recordings on the real devices. Time between the screen flash and the tone: changed by -14.1 ms and by -12.1 ms from the first to the last markers. Memory: -29.9 % and -0.4 % growth (limit +5 %). Section 3. |
| Killing the app mid-recording leaves a recoverable, playable file | **Met** | Kill test with the real FFmpeg (5 of 5 runs, M1) and, on the real app, a hard kill during a recording, then recovery through the recovery dialog: 12.2 s recovered, all three tracks within 20 ms of each other, `recovered: true`. |
| Unplugging the microphone does not stop the recording | **Not checked on real hardware** | Passes with a synthetic microphone (`session_e2e`, `recorder_e2e`). The real check, `src-tauri/tests/mic_unplug.rs`, is written and needs an external USB or 3.5 mm microphone and a person to unplug it. |
| The control window is not in the recorded video | **Met** | `scripts/manual-checks/check-control-window-hidden.ps1`: the control window is painted solid green and is on the screen (position 800,12, 336x93, capture exclusion set, page colour read back) while recording 8 s; 159 frames scanned for green in that place: none. The scanner has a self-test that proves it finds green when it is there. |

## 2. The machine

Everything below ran on **IV-LT-Han**: Windows 11 Enterprise build 26200, Intel Core Ultra 7 265U (12 cores, 14
threads), 31.5 GB RAM, Intel graphics with a hardware H.264 encoder (`h264_mf`, hardware), built-in SoundWire microphone
and speakers (CS42L43), two monitors connected at the time (HP 27xw, main, and DELL S2719NX, both 1920x1080; the
built-in screen was off). This is **faster than the reference machine** (Core i5 or Ultra 5, 16 GB, integrated graphics
only), so CPU numbers here are on the optimistic side. Nothing was measured on the reference class of machine.

## 3. Measurements

Both long runs: default setup (main monitor, microphone, system audio, 1080p, 15 fps), the app's own `Recorder`, the
machine left alone, a marker every 60 s (a 0.4 s magenta window on the screen and a 1 kHz tone at -30 dBFS together).
The markers are found in the saved files: the flash in the video, the tone in the system-audio track.

| | Run 1 | Run 2 |
|---|---|---|
| Code | before the capture fixes of Section 4 | with the OS frame throttle and frame timestamps |
| Recorded | 7200.514 s | 7201.633 s |
| Warnings, gaps, errors | 0, 0, 0 | 0, 0, 0 |
| Length of video / microphone / system audio vs media | within 16 ms | within 27 ms |
| Video minus audio, mean of first / last markers | 20.0 ms / 5.9 ms | 30.5 ms / 18.4 ms |
| Change (limit 100 ms) | **-14.1 ms** | **-12.1 ms** |
| Median of the same markers | 20 ms / 0 ms | 26.7 ms / 20.0 ms |
| Largest single offset | 910 ms (6 flashes over 150 ms late) | 63 ms |
| Markers whose flash is in the video | 106 of 122 | 120 of 123 |
| Memory (this program plus its FFmpeg processes), minutes 10-15 / last 5 min | 459 / 322 MB (-29.9 %) | 461 / 459 MB (-0.4 %) |
| Peak memory, average CPU | 473 MB, 29 % of one core | 469 MB, 29 % of one core |
| Time to stop and save | 151 s | 134 s |
| Clock error reported (microphone / system audio) | 0.21 / 0.19 ppm | 1.55 / -89.22 ppm |

How to read it:
- The video side can only be read to the 15 fps frame grid (66 ms). To get better than that, every marker starts 0 to 66
  ms later than the one-minute grid, and the mean of 17 to 19 markers is compared (the median is listed next to it).
- Run 2's -89 ppm for the system audio was not investigated. The track still matches the media length within 27 ms and
  the offsets stay put, so it had no effect. (One 10 ms hiccup inside the last two-minute measurement window would read
  like that; this is a guess, not a finding.)
- Saving a 2-hour recording takes 2 to 2.5 minutes (the tracks are put together); the app shows "Saving the recording"
  and quitting waits for it. A 40-minute recording took 26 s.

Also checked:
- **One minute on each monitor** (hardware smoke test): HP 27xw 60.54 s, DELL S2719NX 60.30 s; every track within 0.25 s of
  the media; hardware encoder; no gaps or errors; CPU 25 % and 19 % of one core; memory peak 451 and 443 MB. The built-in
  screen was off, so two monitors were recorded, not three.
- The other hardware smoke tests (8 in all): devices and capabilities, a real recording with pixel and loudness checks,
  the app's window disappearing from the video once excluded, pause with real devices, two recordings in a row.

## 4. What the real hardware showed, and what was fixed

Found by recording on the real devices and driving the real app; each is in the commit that fixed it.
1. **A gap read as drift** (M3). After a pause the devices restart about 0.5 s late; that silence was counted as a clock
   running 1.3 % slow, the audio was stretched, and 71 "repeated audio" errors followed. A gap now restarts the drift
   measurement, and an estimate no device could produce (over 2000 ppm) is never applied. Reproduced by unit tests first.
2. **The preview could end up off** (M3). Start, stop and start were sent at once and carried out in any order. They now
   go through one queue.
3. **Screen states lost** (M4). The capture handler skipped frames that arrived within half a frame interval of the last
   one; a skipped frame is a state of the screen that is never recorded if nothing changes afterwards. In run 1, 13 % of
   0.4 s flashes never reached the video. Windows is now asked to throttle (it delivers the newest state) and the handler
   skips nothing. In a 5-minute run with a marker every 3 s: 86 of 95 flashes before, 94 of 94 after.
4. **Frames stamped with their arrival time** (M4). Frames arriving up to 0.9 s late put what they showed at the wrong
   time. Each frame is now stamped with its own capture time.
5. **A frame that finds the recorder's queue full was thrown away** (M4). The newest one is now kept and sent again as
   soon as there is room (`FrameRelay`, unit-tested). The queue does fill: in a 40-minute run it happened 30 times (the
   recorder logs it), where before each of those lost the newest state of the screen. See "Final validation".
6. **The default storage folder was inside OneDrive** here, which uploaded a recording within minutes. The default is now
   `Downloads\MeetingMind` (a user decision), and the Record screen and Settings warn if the chosen folder is under
   OneDrive, Dropbox, Google Drive or iCloud Drive.

### Final validation (code with all of the above)

The 2-hour runs of Section 3 were made before fix 5. The final code was run for 20 and for 40 minutes with a marker every
3 s (about 1200 markers), because that is how often the rare losses can be counted:

| Run | Markers whose flash is in the video | Largest offset | Change, first to last markers | Memory growth |
|---|---|---|---|---|
| 2 h, run 2 (fixes 3 and 4, no relay), a marker per minute | 120 of 123 (97.6 %) | 63 ms | -12.1 ms | -0.4 % |
| 5 min, before fix 3, a marker per 3 s | 86 of 95 (90.5 %) | 93 ms | | |
| 5 min, with fix 3 and 4 | 94 of 94 | 73 ms | -15.6 ms | |
| 20 min, with the relay | 393 of 393 | one at 953 ms, the rest under 120 ms | -18.8 ms | |
| **40 min, with the relay** | **787 of 788 (99.9 %)** | none beyond 100 ms | **+1.3 ms** | **-0.5 %** |

In the 40-minute run the recorder's queue was full 30 times and no frame was lost; showing the test window never took
longer than 100 ms. At the 2.4 % loss seen before the relay, about 19 of those 788 markers would have been missing; 1 was.
That is the evidence that the relay removes the loss.

**Not fully explained:** one flash of 788 is still missing, and one of 393 (the 20-minute run) appeared 0.95 s late.
No gap or error goes with them. They may be the desktop not composing the test's 0.4 s window in time (a limit of the
test), but that was not shown. The soak test fails if fewer than 95 % of the markers reach the video, which catches the
defect of fix 3 (87 to 91 %) without claiming to remove what is not understood.

The final code was not run for the full two hours: the acceptance numbers of Section 3 are from the two earlier
2-hour runs plus these shorter ones. A third 2-hour run on the final code costs 2 hours of an idle machine and is not
expected to change the result; it can be done on request.

## 5. What works

Everything in the Phase 2 scope on Windows: screen, microphone and system audio recording with one clock; pause and
resume; crash-safe files and recovery; own-window exclusion only while recording; sleep prevention; device-change and
disk-space handling; the Record screen, control window, tray, shortcuts, consent dialog, warnings, recovery prompt and
playback. Details of M3 are in phase-2-plan.md ("What M3 settled").

## 6. Tests

- Rust: 207 unit tests, session end to end (14), recorder end to end (14), preview (3), recovery with a killed process
  (5), real FFmpeg (8), logging and log guard, with the real FFmpeg and synthetic devices. CI runs them on Windows with
  `REQUIRE_FFMPEG`; a skipped test never counts as a pass.
- Frontend: 132 Vitest tests.
- On the real devices, run by hand: 8 hardware smoke tests, the soak (`tests/soak.rs`), the control-window check
  (`scripts/manual-checks`), and driving the real app (shortcuts, tray window, consent dialog, preview, playback,
  recovery), none of which CI can do.
- CI: green on Windows and macOS for the final commit (4533d81, run 39020527704). One earlier run failed on a timing
  assumption in a synthetic-microphone test (it expected the unplug at 2.0 s after the start, and starting the session
  took most of a second on a loaded runner); the test now reads the real media time, and nothing in the recorder changed.

## 7. What does not work, and what is blocked

- **Blocked on hardware: the microphone-unplug check on real hardware.** Needs an external USB or 3.5 mm microphone
  (a headset works) and someone to unplug and replug it when the test asks. Run: `cargo test --test mic_unplug --
  --ignored --nocapture` from `src-tauri/` with the microphone connected.
- **Not exercised on a real desktop:** clicking the tray menu entries and seeing the icon states; Quit and hide-on-close
  while recording; a shortcut already owned by another program; Cancel in the native consent dialog.
- **Known limit:** after a pause, the microphone and system-audio tracks are silent for about 0.5 s while the devices
  restart; it is listed in `meeting.json` as a gap.
- **Residual loss of rare short screen states** (Section 4, "Not fully explained").
- macOS: only stubs; compiled and linted in CI (Phase 5).
- The installer: `pnpm tauri build` cannot extract the NSIS tool on the development computer (a company allow-list
  blocks it); CI builds the installer. FFmpeg is bundled as a Windows sidecar; the installed app was not run from the
  installer.
- Distribution of an H.264 encoder (patent licensing) and the "needs review" licences in `LICENSES.md` are not decided.
