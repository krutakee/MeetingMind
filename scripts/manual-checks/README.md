# Manual checks on a real desktop

These need a Windows computer with a screen, microphone and speakers, and a person who leaves the keyboard alone while
they run. CI cannot run them. Each prints what it did and ends with PASSED or FAILED.

| Script | What it checks |
|---|---|
| `check-control-window-hidden.ps1` | The control window is not in the recorded video (painted solid green, then the video is scanned). |
| `scan-frames-for-colour.mjs` | The scanner the check uses. `--self-test` proves it finds the colour when it is there. |
| `keys.ps1`, `windows-of.ps1`, `cdp.mjs` | Helpers: send the global shortcuts, list a process's windows (position, size, capture exclusion), and read or change the app's own web pages over the DevTools protocol. |

The soak (2-hour drift and memory) and the microphone unplug check are Rust tests: `src-tauri/tests/soak.rs` and
`src-tauri/tests/mic_unplug.rs` (their headers say how to run them).

Notes:
- `cdp.mjs` only touches pages that are MeetingMind's own (it checks the address). Another program may listen on a
  debugging port too: on the development computer, port 9222 belonged to a media player. Use a port nothing else uses.
- A recording made by a check is saved in the storage folder and listed in the app. To remove one: delete its folder,
  and delete its row from the `meetings` table of `%APPDATA%\com.meetingmind.desktop\meetingmind.db` with the app closed
  (for example with Python's `sqlite3`: `DELETE FROM meetings WHERE folder_path LIKE '%<folder name>'`).
