# `meeting.json` (schema version 1)

Written into every meeting folder (SPEC Section 7). It says what happened while recording: devices used, pauses,
gaps, errors. It never contains transcript text. Source of truth: `src-tauri/src/recording/metadata.rs`.

The file is rewritten while the recording runs (write a temporary file, then rename it over the old one), so after a
crash it describes everything up to the last update. Readers must ignore fields they do not know. A reader that finds a
`schemaVersion` higher than it understands must refuse the file.

Times in the file are milliseconds on the **media timeline**: position in the saved files, with paused time already
removed. Wall-clock times are RFC 3339 in UTC.

```json
{
  "schemaVersion": 1,
  "appVersion": "0.1.0",
  "meetingId": 7,
  "title": "Weekly sync",
  "mode": "screenAudio",
  "startedAt": "2026-10-16T09:05:03.250Z",
  "endedAt": "2026-10-16T10:02:44.810Z",
  "durationMs": 3320000,
  "status": "complete",
  "recovered": false,
  "video": { "monitor": "\\\\.\\DISPLAY2", "width": 1920, "height": 1080, "fps": 15, "encoder": "h264_mf (hardware)" },
  "microphone": { "device": "Microphone Array", "sampleRate": 48000, "channels": 2, "startOffsetMs": 38, "driftPpm": 0.1 },
  "systemAudio": { "device": "Speakers", "sampleRate": 48000, "channels": 2, "startOffsetMs": 21, "driftPpm": null },
  "pauses": [ { "atMediaMs": 60000, "wallLengthMs": 15000 } ],
  "gaps": [ { "stream": "microphone", "startMs": 90000, "endMs": 95000, "reason": "deviceLost" } ],
  "errors": [ { "atMediaMs": 95000, "stream": "microphone", "kind": "deviceLost", "message": "The microphone was unplugged." } ]
}
```

## Fields

| Field | Type | Meaning |
|---|---|---|
| `schemaVersion` | number | Always 1 for this layout. |
| `appVersion` | string | MeetingMind version that wrote the file. |
| `meetingId` | number or null | Row id in the app database, once it exists. |
| `title` | string | The title the user gave. |
| `mode` | `"screenAudio"` or `"audioOnly"` | |
| `startedAt`, `endedAt` | string, null | Start and end, UTC. `endedAt` is null until the recording ends. |
| `durationMs` | number or null | Length of the saved media. Null until the recording ends. |
| `status` | string | `recording`, `finalizing`, `complete`, `failed` or `cancelled`. |
| `recovered` | boolean | True if the app found this recording unfinished after a crash and recovered it. |
| `video` | object or null | Monitor device name, pixel size, frame rate, encoder used. Null in audio-only mode. |
| `microphone`, `systemAudio` | object or null | Device name, sample rate, channels, `startOffsetMs` (silence added at the start because the device delivered its first data late), `driftPpm` (measured device clock error, or null if not measured). Null if that source was off. |
| `pauses` | array | Each pause: `atMediaMs` (the cut point in the saved media) and `wallLengthMs` (how long it lasted; this time is not in the files). |
| `gaps` | array | Stretches where a stream has silence (audio) or black (video) instead of real content. |
| `errors` | array | Problems that did not stop the recording. |

### Gap reasons

`deviceLost` (microphone, headset or output unplugged or failed), `screenLocked`, `monitorLost`, `sourceStalled`
(a source stopped delivering without saying why), `droppedFrames` (the video encoder could not keep up),
`encoderFailed`.

Short device start-up delays (under 250 ms) are not listed as gaps; they appear only as `startOffsetMs`.

### Streams

`video`, `microphone`, `systemAudio`.

## Compatibility rules

- Adding a field is not a breaking change. Removing or renaming one is, and raises `schemaVersion`.
- The meeting folder also holds the media files and, from Phase 4, `transcript.json`. `meeting.json` does not
  duplicate the transcript.
