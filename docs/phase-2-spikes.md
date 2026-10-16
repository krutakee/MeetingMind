# Phase 2, M0 spike results

Measured 2026-10-16 on **IV-LT-Han**: Windows 11 Enterprise build 26200, Core Ultra 7 265U (14 threads), 31.5 GB RAM,
Intel integrated graphics, 3 monitors, built-in SoundWire microphone array and speakers. This machine is faster than the
SPEC reference machine, so CPU figures are lower bounds for that class, not reference-machine results.

The spike programs are in `spikes/phase2/` (throwaway; not part of the product or CI). Every number below came from a
real run; nothing was estimated unless it says so.

## 1. FFmpeg (pinned LGPL build)

| Item | Result |
|---|---|
| Release | BtbN/FFmpeg-Builds tag `autobuild-2026-10-16-13-04`, asset `ffmpeg-n9.0.2-28-g669f5ab083-win64-lgpl-9.0.zip` |
| Zip SHA-256 | `fe7aab7fb1e83b4be657ae46d949a01812cabc60d35766d51fad8dcf97c69319` (matches the release's `checksums.sha256`) |
| `ffmpeg.exe` SHA-256 | `c47e9873edf44d2df1af10c7f509ec9d79626d173ddcda47a2d2a963d22ff70a` |
| Size | `ffmpeg.exe` is 134 MB (static build with every library). Too big to ship; see the plan's risk 2. |
| License | LGPL **v3** (`--enable-version3`). No `--enable-gpl`, no `--enable-nonfree`; `libx264`, `libx265`, `fdk-aac` disabled. |
| Encoders present | `h264_mf`, `libopenh264`, `h264_qsv`, `h264_nvenc`, `h264_amf`, `aac`, `flac`, `libopus`. Muxers `mp4`, `flac`. |
| Runs here | Yes. Not blocked by allow-listing from `C:\projects\`. |
| Retention risk | BtbN keeps dated releases only for a limited time (5 daily releases were listed). The pinned asset may disappear; the plan needs a copy we control (decision for M1). |

### H.264 encoder cost (15 s of 1080p15 synthetic video, bitrate 3 Mbps, CPU seconds per second of video)

| Encoder | CPU-s per video-s | Notes |
|---|---|---|
| `h264_mf` default | 0.42 | Picks the generic "H264 Encoder MFT", not the GPU. By its name and its CPU cost this is the software encoder built into Windows (inferred, not confirmed). |
| `h264_mf -hw_encoding 1` | 0.11 | Picks **Intel Quick Sync H.264 Encoder MFT**. Needs **NV12 input**; with other input formats it fails with "format negotiation failed". |
| `libopenh264` | 0.12 to 0.19 | Software. Cheaper than Microsoft's software encoder. |
| `h264_qsv` | 0.21 | Works; needs the Intel runtime. Not planned (vendor-specific). |

Converting captured BGRA frames to NV12 (needed for the hardware encoder) costs about 0.11 CPU-s per video-s at 1080p15.
Raw BGRA frames piped through stdin into `h264_mf -hw_encoding 1` produced a correct 30-frame, 2.00 s fragmented MP4.
The test video is a synthetic pattern, so real screen content will differ; static screens are cheaper.

### Crash behaviour (process hard-killed 9 s into a 60 s recording)

| Output | Result |
|---|---|
| Ordinary MP4 | Unrecoverable: `moov atom not found`. |
| Fragmented MP4, `h264_mf`, default flags | Not decodable (empty extradata in the initial header). |
| Fragmented MP4 with **`-movflags +frag_keyframe+empty_moov+default_base_moof+delay_moov -flush_packets 1`** | Decodes cleanly to 8.00 s of a 9 s run (loses at most the last keyframe interval). Same with `libopenh264`. |
| FLAC, default buffering | Kept 1 MB (about 8 s of noise); last frame cut ("invalid residual"). |
| FLAC with **`-flush_packets 1`** | Decodes cleanly through the kill. |
| After recovery | `-c copy` remux rewrites the FLAC header and turns a fragmented MP4 into a normal one; the final mix-and-mux then works. |

A recovered recording's tracks end at different moments (video up to about 2 s short). Recovery must trim to the
common end, otherwise the audio is longer than the video.

### Final mux speed

Mixing two 10-minute tracks to AAC took 7.0 s wall (12.6 CPU-s). **Extrapolated, not measured:** about 85 s for 2 hours.

## 2. Screen capture (`windows-capture` 2.0.1, Windows.Graphics.Capture)

| Observation | Result |
|---|---|
| First frame | 145 to 184 ms after capture starts, even on a static screen. |
| Frame delivery | Only when something changes. Busy monitor: 33.5 fps (flashing test window), median interval 16.7 ms. Idle-looking monitors: 1.1 fps (primary), 0.2 fps (built-in panel, where the gap between two frames reached **9.55 s**), 10.7 fps (a monitor with some activity). The video pacer must repeat the last frame to keep a constant frame rate. |
| Frame timestamps vs wall clock | Arrival minus timestamp stayed within -10 to +13 ms. |
| Copy of a 1080p frame to CPU memory | Median 3.1 to 3.9 ms, p95 about 5 ms. |
| Sizes | Physical pixels: the built-in panel is 1920x1200 although Windows reports 1536x960 after scaling. |
| Monitor numbering | `Monitor::from_index(n)` does not match the `index()` shown in the enumeration or the `DISPLAYn` names (index 2 turned out to be `DISPLAY3`). Monitors must be chosen by device name or handle from `Monitor::enumerate()`, never by index. |
| Border setting | `DrawBorderSettings::WithoutBorder` starts without error. Whether the border is really hidden was not observed. |

## 3. Audio (`cpal` 0.18.2 on WASAPI)

The master branch on GitHub has a newer API than the 0.18.2 release (`CallbackInfo` vs `InputCallbackInfo`; overruns as
`ErrorKind::Xrun` through the error callback in 0.18.2). Code must be written against the released crate.

| Observation | Result |
|---|---|
| Devices | Microphone and loopback both open at 48 kHz, 2 channels, f32. Loopback is an input stream built on an output device. |
| Start-up | First data 18 to 42 ms after `play()`. |
| Loopback with a silent render stream running | 100 callbacks per second, signal level correct (a -40 dBFS test tone arrived as peak 0.01000). |
| **Loopback with nothing rendering** | **0 callbacks in 8 s.** Loopback gives no data while no program plays audio. |
| 10-minute run | Mic: 59,999 callbacks, 28,799,520 frames (599.99 s). Loopback: 59,998 callbacks, 28,799,040 frames. No lost frames. |
| Clock drift over 10 minutes (regression over all callbacks) | Device clock vs wall clock: microphone **+0.1 ppm**, loopback **-0.2 ppm**. Device clock vs its own timestamps: -0.2 ppm. |
| Callback jitter | Largest gap between callbacks 21 to 22 ms (nominal 10 ms). |
| Overruns | 1 to 2 (mic) and 1 (loopback) in runs of 12 s, 20 s and 10 minutes. The count did not grow with run length, so they look like start-up events. Not confirmed. |
| Timestamps | WASAPI timestamps come from `QueryPerformanceCounter`; they tracked the wall clock within a few ppm. |

The built-in SoundWire audio on this machine is locked to the system clock. USB and Bluetooth devices usually are not,
so the adaptive resampler stays in the plan and must be measured with the external microphone that the unplug test needs.
Audio content was never saved; only counts, timings and signal level.

## 4. What changes in the plan

1. **Encoder order:** `h264_mf` with `-hw_encoding 1` and NV12 input first; then `libopenh264`; the software mode of
   `h264_mf` last. (Approved decision 2 said "h264_mf first, OpenH264 second". The measurement shows `h264_mf` only uses
   the GPU when asked to, and its software mode costs about 3.5 times as much as OpenH264. The first choice is unchanged.)
2. **Muxer flags:** fragmented MP4 needs `+delay_moov`; every track needs `-flush_packets 1`.
3. **Loopback:** keep a silent render stream open on the same output device while recording, and fill any remaining
   gap with silence by clock.
4. **Monitors:** select by device name or handle, report physical pixel sizes.
5. **Recovery:** trim all tracks to the shortest common end.
6. **Drift:** not a problem on the built-in devices (about 0.2 ppm); still to be measured on a USB microphone.
7. **Final mux:** about 85 s for a 2-hour meeting (extrapolated).
8. **License:** FFmpeg is LGPL v3, not v2.1.

## 5. Reproducing

```
cd spikes/phase2 && cargo build --release
target\release\wgc.exe <monitor index> <seconds> [border|noborder]
target\release\audio.exe <seconds> [tone] [norender]
```
FFmpeg is downloaded into `spikes/phase2/downloads/` (git-ignored) with `gh release download` using the tag and asset above.
