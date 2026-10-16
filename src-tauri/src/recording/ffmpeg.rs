//! The FFmpeg sidecar: finding it, refusing a GPL build, choosing the H.264 encoder, running one process per
//! stream, and the final mix-and-mux. Settings here come from the M0 measurements (docs/phase-2-spikes.md).
//!
//! One FFmpeg process per stream (video, microphone, system audio) means a failed encoder cannot stop the others, each
//! track is crash-safe on its own, and the mix is built afterwards from the lossless tracks.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::system::process::no_console_window;

/// File names the bundled sidecar can have next to the app executable.
const SIDECAR_NAMES: &[&str] = &["ffmpeg.exe", "ffmpeg"];
/// Tauri's sidecar naming, as produced by `scripts/fetch-ffmpeg.mjs`.
#[cfg(any(test, debug_assertions))]
const DEV_SIDECAR_NAMES: &[&str] = &[
    "ffmpeg-x86_64-pc-windows-msvc.exe",
    "ffmpeg-aarch64-apple-darwin",
];
const ENV_OVERRIDE: &str = "MEETINGMIND_FFMPEG";

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error("FFmpeg was not found (looked in: {})", .tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    NotFound { tried: Vec<PathBuf> },
    #[error("FFmpeg could not be started ({0}): {1}")]
    Spawn(PathBuf, io::Error),
    #[error("this FFmpeg build is not allowed: {0}")]
    ForbiddenBuild(String),
    #[error("no H.264 encoder works on this computer")]
    NoVideoEncoder,
    #[error("{what} failed (exit code {code:?}): {stderr}")]
    Failed {
        what: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("{0} did not finish in time")]
    Timeout(String),
    #[error("nothing to write: {0}")]
    InvalidInputs(&'static str),
    #[error("FFmpeg input/output error: {0}")]
    Io(#[from] io::Error),
}

// ---------------------------------------------------------------------------------------------------------------
// Finding and checking the binary

/// Where to look, in order: the `MEETINGMIND_FFMPEG` override, next to the app executable (where Tauri places the
/// bundled sidecar), and, in development and test builds only, `src-tauri/binaries/` (filled by `pnpm fetch:ffmpeg`).
pub fn locate() -> Result<PathBuf, FfmpegError> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    #[cfg(any(test, debug_assertions))]
    let dev_dir = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    #[cfg(not(any(test, debug_assertions)))]
    let dev_dir: Option<PathBuf> = None;
    locate_in(
        std::env::var_os(ENV_OVERRIDE),
        exe_dir.as_deref(),
        dev_dir.as_deref(),
    )
}

pub fn locate_in(
    env_override: Option<OsString>,
    exe_dir: Option<&Path>,
    dev_dir: Option<&Path>,
) -> Result<PathBuf, FfmpegError> {
    let mut tried = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env_override {
        candidates.push(PathBuf::from(path));
    }
    if let Some(dir) = exe_dir {
        candidates.extend(SIDECAR_NAMES.iter().map(|name| dir.join(name)));
    }
    #[cfg(any(test, debug_assertions))]
    if let Some(dir) = dev_dir {
        candidates.extend(DEV_SIDECAR_NAMES.iter().map(|name| dir.join(name)));
    }
    #[cfg(not(any(test, debug_assertions)))]
    let _ = dev_dir;
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
        tried.push(candidate);
    }
    Err(FfmpegError::NotFound { tried })
}

#[derive(Debug, Clone)]
pub struct FfmpegInfo {
    pub path: PathBuf,
    pub version: String,
    pub configuration: String,
}

fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    no_console_window(&mut command);
    command.stdin(Stdio::null());
    command
}

/// MeetingMind ships FFmpeg as an LGPL build only (CLAUDE.md). A build configured with GPL or non-free components
/// is refused, so one can never be shipped or used by accident.
pub fn check_license(configuration: &str) -> Result<(), FfmpegError> {
    for forbidden in ["--enable-gpl", "--enable-nonfree"] {
        if configuration
            .split_whitespace()
            .any(|flag| flag == forbidden)
        {
            return Err(FfmpegError::ForbiddenBuild(format!(
                "it was configured with {forbidden}"
            )));
        }
    }
    Ok(())
}

/// Runs `ffmpeg -version`, reads the build configuration, and applies the license check.
pub fn probe(path: &Path) -> Result<FfmpegInfo, FfmpegError> {
    let output = command(path)
        .args(["-hide_banner", "-version"])
        .output()
        .map_err(|e| FfmpegError::Spawn(path.to_path_buf(), e))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.lines().next().unwrap_or("").to_string();
    if !output.status.success() || !version.starts_with("ffmpeg version") {
        return Err(FfmpegError::Failed {
            what: "ffmpeg -version".into(),
            code: output.status.code(),
            stderr: tail(&String::from_utf8_lossy(&output.stderr)),
        });
    }
    let configuration = text
        .lines()
        .find_map(|line| line.strip_prefix("configuration:"))
        .unwrap_or("")
        .trim()
        .to_string();
    check_license(&configuration)?;
    Ok(FfmpegInfo {
        path: path.to_path_buf(),
        version,
        configuration,
    })
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(8)..].join(" | ")
}

// ---------------------------------------------------------------------------------------------------------------
// Encoder choice and command-line arguments

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Kind {
    /// Media Foundation, asking for a hardware encoder (Intel Quick Sync, AMD, NVIDIA). Cheapest on the CPU.
    HardwareMf,
    /// Cisco OpenH264, software.
    OpenH264,
    /// Media Foundation without asking for hardware: Windows' own software encoder, the most expensive of the three.
    SoftwareMf,
}

impl H264Kind {
    /// Tried in this order (M0: hardware 0.11 CPU-s per video-s, OpenH264 0.12 to 0.19, software MF 0.42).
    pub const PREFERENCE: [H264Kind; 3] = [
        H264Kind::HardwareMf,
        H264Kind::OpenH264,
        H264Kind::SoftwareMf,
    ];

    fn codec_args(self) -> Vec<&'static str> {
        match self {
            H264Kind::HardwareMf => vec!["-c:v", "h264_mf", "-hw_encoding", "1"],
            H264Kind::OpenH264 => vec!["-c:v", "libopenh264"],
            H264Kind::SoftwareMf => vec!["-c:v", "h264_mf"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            H264Kind::HardwareMf => "h264_mf (hardware)",
            H264Kind::OpenH264 => "libopenh264",
            H264Kind::SoftwareMf => "h264_mf (software)",
        }
    }
}

/// Picks the first H.264 encoder in `H264Kind::PREFERENCE` that can actually encode a few test frames here.
pub fn select_h264(ffmpeg: &FfmpegInfo) -> Result<H264Kind, FfmpegError> {
    for kind in H264Kind::PREFERENCE {
        let status = command(&ffmpeg.path)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=640x360:rate=15,format=nv12")
            .args(["-frames:v", "15"])
            .args(kind.codec_args())
            .args(["-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| FfmpegError::Spawn(ffmpeg.path.clone(), e))?;
        if status.success() {
            return Ok(kind);
        }
    }
    Err(FfmpegError::NoVideoEncoder)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// Arguments for the video process: raw BGRA frames on stdin, a fragmented MP4 out.
///
/// The muxer flags matter (M0): `delay_moov` is required with the hardware encoder or a hard kill loses the whole
/// file, and `-flush_packets 1` makes every packet reach the disk. Keyframes every 2 s bound what a crash can lose.
pub fn video_args(kind: H264Kind, params: &VideoParams, output: &Path) -> Vec<OsString> {
    let mut args = os(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-video_size",
    ]);
    args.push(format!("{}x{}", params.width, params.height).into());
    args.extend(os(&["-framerate"]));
    args.push(params.fps.to_string().into());
    // NV12 is what the hardware encoder accepts (M0: other input formats fail to negotiate).
    args.extend(os(&["-i", "pipe:0", "-vf", "format=nv12"]));
    args.extend(kind.codec_args().into_iter().map(OsString::from));
    args.extend(os(&["-b:v"]));
    args.push(params.bitrate_bps.to_string().into());
    args.extend(os(&["-g"]));
    args.push((params.fps * 2).to_string().into());
    args.extend(os(&[
        "-flush_packets",
        "1",
        "-movflags",
        "+frag_keyframe+empty_moov+default_base_moof+delay_moov",
    ]));
    args.push(output.into());
    args
}

/// Arguments for an audio track process: interleaved f32 little-endian samples on stdin, 24-bit FLAC out.
pub fn audio_track_args(sample_rate: u32, channels: u16, output: &Path) -> Vec<OsString> {
    let mut args = os(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "f32le",
        "-ar",
    ]);
    args.push(sample_rate.to_string().into());
    args.extend(os(&["-ac"]));
    args.push(channels.to_string().into());
    args.extend(os(&[
        "-i",
        "pipe:0",
        "-c:a",
        "flac",
        "-sample_fmt",
        "s32",
        "-bits_per_raw_sample",
        "24",
        "-flush_packets",
        "1",
    ]));
    args.push(output.into());
    args
}

// ---------------------------------------------------------------------------------------------------------------
// One running FFmpeg process fed through a bounded queue

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Queued,
    /// The queue was full (the encoder is slower than the source); this item was discarded.
    Dropped,
    /// The process has failed; nothing more will be accepted.
    EncoderFailed,
}

pub struct StreamEncoder {
    name: String,
    child: Child,
    tx: Option<SyncSender<Arc<Vec<u8>>>>,
    writer: Option<JoinHandle<io::Result<()>>>,
    stderr_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    dropped: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    finished: bool,
}

impl StreamEncoder {
    /// Starts FFmpeg with `args`, reading stdin through a queue of `queue_len` items (so memory stays bounded).
    pub fn spawn(
        name: &str,
        ffmpeg: &Path,
        args: &[OsString],
        queue_len: usize,
    ) -> Result<Self, FfmpegError> {
        let mut cmd = command(ffmpeg);
        cmd.args(args);
        Self::spawn_command(name, cmd, queue_len)
    }

    /// Like `spawn`, for any program that reads stdin (tests use a stand-in that does not read, to fill the queue).
    pub fn spawn_command(
        name: &str,
        mut cmd: Command,
        queue_len: usize,
    ) -> Result<Self, FfmpegError> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let program = PathBuf::from(cmd.get_program());
        let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn(program, e))?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let (tx, rx) = sync_channel::<Arc<Vec<u8>>>(queue_len.max(1));
        let failed = Arc::new(AtomicBool::new(false));
        let failed_in_writer = failed.clone();
        let writer = std::thread::Builder::new()
            .name(format!("{name}-writer"))
            .spawn(move || -> io::Result<()> {
                let mut stdin = stdin;
                while let Ok(chunk) = rx.recv() {
                    if let Err(e) = stdin.write_all(chunk.as_slice()) {
                        failed_in_writer.store(true, Ordering::SeqCst);
                        return Err(e); // dropping rx makes every later try_send report EncoderFailed
                    }
                }
                Ok(()) // dropping stdin closes the pipe, which tells FFmpeg the input has ended
            })?;

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail_in_reader = stderr_tail.clone();
        let stderr_reader = std::thread::Builder::new()
            .name(format!("{name}-stderr"))
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut lines = tail_in_reader.lock().unwrap();
                    lines.push_back(line);
                    if lines.len() > 12 {
                        lines.pop_front();
                    }
                }
            })?;

        Ok(Self {
            name: name.to_string(),
            child,
            tx: Some(tx),
            writer: Some(writer),
            stderr_reader: Some(stderr_reader),
            stderr_tail,
            dropped: Arc::new(AtomicU64::new(0)),
            failed,
            finished: false,
        })
    }

    /// Queues data without ever blocking. Accepts a `Vec<u8>` or a shared `Arc<Vec<u8>>` (a repeated video frame is
    /// queued many times without being copied).
    pub fn try_send(&self, data: impl Into<Arc<Vec<u8>>>) -> SendOutcome {
        let outcome = self.offer(data.into());
        if outcome == SendOutcome::Dropped {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        outcome
    }

    fn offer(&self, data: Arc<Vec<u8>>) -> SendOutcome {
        let Some(tx) = &self.tx else {
            return SendOutcome::EncoderFailed;
        };
        match tx.try_send(data) {
            Ok(()) => SendOutcome::Queued,
            Err(TrySendError::Full(_)) => SendOutcome::Dropped,
            Err(TrySendError::Disconnected(_)) => SendOutcome::EncoderFailed,
        }
    }

    /// Queues data, waiting for room for as long as `keep_waiting(time waited so far)` says yes. For worker threads
    /// that must not lose an item (a video slot must not be skipped or the video would drift short); never use it on
    /// a device callback. A slow encoder is not a dead one, so the caller decides how long is too long. Returns
    /// `Dropped` only if it gave up, and `EncoderFailed` as soon as the process is gone.
    pub fn send_while(
        &self,
        data: impl Into<Arc<Vec<u8>>>,
        mut keep_waiting: impl FnMut(Duration) -> bool,
    ) -> SendOutcome {
        let data = data.into();
        let started = Instant::now();
        loop {
            match self.offer(data.clone()) {
                SendOutcome::Dropped => {
                    if !keep_waiting(started.elapsed()) {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                        return SendOutcome::Dropped;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                other => return other,
            }
        }
    }

    /// Items discarded because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// True once writing to the process has failed (it died or closed its input).
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    fn stderr_text(&self) -> String {
        let lines = self.stderr_tail.lock().unwrap();
        lines.iter().cloned().collect::<Vec<_>>().join(" | ")
    }

    /// Lets the queue drain, ends the input, and waits up to `timeout` for the process to finish. A process that
    /// does not finish is killed. Returns an error if it exited unsuccessfully.
    pub fn finish(mut self, timeout: Duration) -> Result<(), FfmpegError> {
        self.finished = true;
        drop(self.tx.take()); // the writer drains what is queued, then closes stdin
        let deadline = Instant::now() + timeout;
        let status = loop {
            match self.child.try_wait()? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    self.join_threads();
                    return Err(FfmpegError::Timeout(self.name.clone()));
                }
                None => std::thread::sleep(Duration::from_millis(15)),
            }
        };
        self.join_threads();
        if status.success() {
            Ok(())
        } else {
            Err(FfmpegError::Failed {
                what: self.name.clone(),
                code: status.code(),
                stderr: self.stderr_text(),
            })
        }
    }

    /// Stops the process at once, without waiting for queued data. Output files are left as they are.
    pub fn abort(mut self) {
        self.finished = true;
        drop(self.tx.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.join_threads();
    }

    fn join_threads(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for StreamEncoder {
    fn drop(&mut self) {
        if !self.finished {
            // Never leave an orphaned FFmpeg behind if the owner is dropped (for example during a panic).
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Whole-file operations: measuring, remuxing, and the final mix-and-mux

/// Reads the last `time=HH:MM:SS.xx` (or `out_time_us=` value) of FFmpeg progress output as a duration.
fn parse_clock(text: &str) -> Option<Duration> {
    let start = text.rfind("time=")? + "time=".len();
    let clock: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.')
        .collect();
    let mut parts = clock.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    let seconds =
        h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    (seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// The length of a media file, found by reading its packets without decoding (fast, and works for fragmented MP4
/// and for FLAC files whose header was never finished).
pub fn measure_duration(ffmpeg: &Path, file: &Path) -> Result<Duration, FfmpegError> {
    let output = command(ffmpeg)
        .args(["-hide_banner", "-v", "error", "-stats", "-i"])
        .arg(file)
        .args(["-c", "copy", "-f", "null", "-"])
        .output()
        .map_err(|e| FfmpegError::Spawn(ffmpeg.to_path_buf(), e))?;
    let stderr = String::from_utf8_lossy(&output.stderr).replace('\r', "\n");
    if !output.status.success() {
        return Err(FfmpegError::Failed {
            what: "reading media length".into(),
            code: output.status.code(),
            stderr: tail(&stderr),
        });
    }
    parse_clock(&stderr).ok_or(FfmpegError::Failed {
        what: "reading media length".into(),
        code: output.status.code(),
        stderr: "no time reported".into(),
    })
}

/// Copies streams into a new container without re-encoding: rewrites a FLAC header, or turns a fragmented MP4
/// into a normal one. Used when recovering a recording after a crash.
pub fn remux_copy(ffmpeg: &Path, input: &Path, output: &Path) -> Result<(), FfmpegError> {
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-c", "copy"]);
    if output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
    {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg(output);
    run_to_completion(cmd, "remux")
}

fn run_to_completion(mut cmd: Command, what: &str) -> Result<(), FfmpegError> {
    let output = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| FfmpegError::Spawn(PathBuf::from(cmd.get_program()), e))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(FfmpegError::Failed {
            what: what.into(),
            code: output.status.code(),
            stderr: tail(&String::from_utf8_lossy(&output.stderr)),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FinalizeInputs<'a> {
    pub video: Option<&'a Path>,
    pub microphone: Option<&'a Path>,
    pub system_audio: Option<&'a Path>,
    pub output: &'a Path,
    /// Cut everything at this length (used by recovery, where the tracks end at different moments).
    pub trim_to: Option<Duration>,
}

/// Arguments for the final run: copy the video, mix the audio tracks into one AAC track.
///
/// Both tracks are brought to stereo, 48 kHz, then summed (no normalization) through a limiter so two loud sources
/// cannot clip. With one audio track there is no mix, only the conversion.
pub fn final_mux_args(inputs: &FinalizeInputs<'_>) -> Result<Vec<OsString>, FfmpegError> {
    let audio: Vec<&Path> = [inputs.microphone, inputs.system_audio]
        .into_iter()
        .flatten()
        .collect();
    if inputs.video.is_none() && audio.is_empty() {
        return Err(FfmpegError::InvalidInputs("no video and no audio tracks"));
    }

    let mut args = os(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostats",
        "-progress",
        "pipe:1",
        "-y",
    ]);
    let mut next_index = 0usize;
    if let Some(video) = inputs.video {
        args.extend(os(&["-i"]));
        args.push(video.into());
        next_index += 1;
    }
    let first_audio_index = next_index;
    for track in &audio {
        args.extend(os(&["-i"]));
        args.push((*track).into());
    }

    if !audio.is_empty() {
        let mut filter = String::new();
        for i in 0..audio.len() {
            let source = first_audio_index + i;
            let label = if audio.len() == 1 {
                "mix".to_string()
            } else {
                format!("a{i}")
            };
            filter.push_str(&format!(
                "[{source}:a]aformat=sample_fmts=fltp:channel_layouts=stereo,aresample=48000[{label}];"
            ));
        }
        if audio.len() > 1 {
            filter.push_str(
                "[a0][a1]amix=inputs=2:normalize=0:duration=longest,alimiter=limit=0.98[mix]",
            );
        } else {
            filter.pop(); // the trailing ';'
        }
        args.extend(os(&["-filter_complex"]));
        args.push(filter.into());
    }

    if inputs.video.is_some() {
        args.extend(os(&["-map", "0:v", "-c:v", "copy"]));
    }
    if !audio.is_empty() {
        args.extend(os(&["-map", "[mix]", "-c:a", "aac", "-b:a", "160k"]));
    }
    args.extend(os(&["-movflags", "+faststart"]));
    if let Some(limit) = inputs.trim_to {
        args.extend(os(&["-t"]));
        args.push(format!("{:.3}", limit.as_secs_f64()).into());
    }
    args.push(inputs.output.into());
    Ok(args)
}

/// Runs the final mix-and-mux. `on_progress` receives how much media has been written so far.
pub fn run_final_mux(
    ffmpeg: &Path,
    inputs: &FinalizeInputs<'_>,
    mut on_progress: impl FnMut(Duration),
) -> Result<(), FfmpegError> {
    let args = final_mux_args(inputs)?;
    let mut child = command(ffmpeg)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FfmpegError::Spawn(ffmpeg.to_path_buf(), e))?;
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let stdout = child.stdout.take().expect("stdout was piped");
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(micros) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            on_progress(Duration::from_micros(micros));
        }
    }
    let status = child.wait()?;
    let stderr_text = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(FfmpegError::Failed {
            what: "final mix".into(),
            code: status.code(),
            stderr: tail(&stderr_text),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn contains_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    // --- license guard ---

    #[test]
    fn an_lgpl_configuration_is_accepted() {
        check_license(
            "--prefix=/ffbuild/prefix --enable-version3 --disable-libx264 --enable-libopenh264",
        )
        .unwrap();
    }

    #[test]
    fn gpl_and_nonfree_configurations_are_refused() {
        for config in [
            "--enable-gpl --enable-libx264",
            "--prefix=/x --enable-nonfree --enable-libfdk-aac",
            "--enable-version3 --enable-gpl",
        ] {
            assert!(
                matches!(check_license(config), Err(FfmpegError::ForbiddenBuild(_))),
                "{config}"
            );
        }
    }

    #[test]
    fn disable_flags_and_lookalikes_do_not_trip_the_guard() {
        check_license("--disable-gpl --disable-nonfree").unwrap();
        check_license("--enable-gpl-compatible-thing").unwrap();
    }

    // --- locating ---

    #[test]
    fn the_override_wins_and_a_missing_one_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = dir.path().join("app");
        std::fs::create_dir(&exe_dir).unwrap();
        std::fs::write(exe_dir.join("ffmpeg.exe"), b"x").unwrap();
        let custom = dir.path().join("custom-ffmpeg");
        std::fs::write(&custom, b"x").unwrap();

        let found = locate_in(Some(custom.clone().into()), Some(&exe_dir), None).unwrap();
        assert_eq!(found, custom);

        let missing = dir.path().join("nope");
        let found = locate_in(Some(missing.clone().into()), Some(&exe_dir), None).unwrap();
        assert_eq!(
            found,
            exe_dir.join("ffmpeg.exe"),
            "falls back to the sidecar next to the app"
        );
    }

    #[test]
    fn a_missing_binary_lists_where_it_looked() {
        let dir = tempfile::tempdir().unwrap();

        let error = locate_in(None, Some(dir.path()), Some(dir.path())).unwrap_err();

        match error {
            FfmpegError::NotFound { tried } => assert!(tried.len() >= 2, "{tried:?}"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // --- arguments ---

    fn params() -> VideoParams {
        VideoParams {
            width: 1920,
            height: 1080,
            fps: 15,
            bitrate_bps: 2_500_000,
        }
    }

    #[test]
    fn video_arguments_carry_the_crash_safety_flags_from_the_spike() {
        let args = text(&video_args(
            H264Kind::HardwareMf,
            &params(),
            Path::new("video.mp4"),
        ));

        assert!(contains_pair(
            &args,
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof+delay_moov"
        ));
        assert!(contains_pair(&args, "-flush_packets", "1"));
        assert!(contains_pair(&args, "-hw_encoding", "1"));
        assert!(contains_pair(&args, "-vf", "format=nv12"));
        assert!(contains_pair(&args, "-video_size", "1920x1080"));
        assert!(contains_pair(&args, "-framerate", "15"));
        assert!(contains_pair(&args, "-g", "30"), "a keyframe every 2 s");
        assert_eq!(args.last().unwrap(), "video.mp4");
    }

    #[test]
    fn only_the_hardware_choice_asks_for_hardware() {
        let hw = text(&video_args(
            H264Kind::HardwareMf,
            &params(),
            Path::new("v.mp4"),
        ));
        let openh264 = text(&video_args(
            H264Kind::OpenH264,
            &params(),
            Path::new("v.mp4"),
        ));
        let software = text(&video_args(
            H264Kind::SoftwareMf,
            &params(),
            Path::new("v.mp4"),
        ));

        assert!(hw.contains(&"-hw_encoding".to_string()));
        assert!(!openh264.contains(&"-hw_encoding".to_string()));
        assert!(contains_pair(&openh264, "-c:v", "libopenh264"));
        assert!(!software.contains(&"-hw_encoding".to_string()));
        assert!(contains_pair(&software, "-c:v", "h264_mf"));
    }

    #[test]
    fn audio_track_arguments_write_flushed_24_bit_flac() {
        let args = text(&audio_track_args(48_000, 2, Path::new("microphone.flac")));

        assert!(contains_pair(&args, "-f", "f32le"));
        assert!(contains_pair(&args, "-ar", "48000"));
        assert!(contains_pair(&args, "-ac", "2"));
        assert!(contains_pair(&args, "-c:a", "flac"));
        assert!(contains_pair(&args, "-bits_per_raw_sample", "24"));
        assert!(contains_pair(&args, "-flush_packets", "1"));
        assert_eq!(args.last().unwrap(), "microphone.flac");
    }

    #[test]
    fn the_final_mux_mixes_two_tracks_through_a_limiter_and_copies_the_video() {
        let inputs = FinalizeInputs {
            video: Some(Path::new("v.mp4")),
            microphone: Some(Path::new("mic.flac")),
            system_audio: Some(Path::new("sys.flac")),
            output: Path::new("recording.mp4"),
            trim_to: None,
        };

        let args = text(&final_mux_args(&inputs).unwrap());

        let filter = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(
            filter.contains("[1:a]aformat") && filter.contains("[2:a]aformat"),
            "inputs are video, mic, system: {filter}"
        );
        assert!(filter.contains("amix=inputs=2:normalize=0"));
        assert!(filter.contains("alimiter"));
        assert!(contains_pair(&args, "-map", "0:v"));
        assert!(contains_pair(&args, "-map", "[mix]"));
        assert!(contains_pair(&args, "-c:v", "copy"));
        assert!(contains_pair(&args, "-c:a", "aac"));
        assert!(!args.contains(&"-t".to_string()));
        assert_eq!(args.last().unwrap(), "recording.mp4");
    }

    #[test]
    fn a_single_audio_track_is_converted_without_a_mix() {
        let inputs = FinalizeInputs {
            video: None,
            microphone: None,
            system_audio: Some(Path::new("sys.flac")),
            output: Path::new("recording.m4a"),
            trim_to: Some(Duration::from_millis(12_345)),
        };

        let args = text(&final_mux_args(&inputs).unwrap());

        let filter = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(
            filter.starts_with("[0:a]aformat") && filter.ends_with("[mix]"),
            "{filter}"
        );
        assert!(!filter.contains("amix"));
        assert!(!args.contains(&"0:v".to_string()));
        assert!(contains_pair(&args, "-t", "12.345"));
    }

    #[test]
    fn video_without_audio_is_just_a_copy() {
        let inputs = FinalizeInputs {
            video: Some(Path::new("v.mp4")),
            microphone: None,
            system_audio: None,
            output: Path::new("recording.mp4"),
            trim_to: None,
        };

        let args = text(&final_mux_args(&inputs).unwrap());

        assert!(!args.contains(&"-filter_complex".to_string()));
        assert!(contains_pair(&args, "-c:v", "copy"));
        assert!(!args.contains(&"aac".to_string()));
    }

    #[test]
    fn nothing_to_write_is_an_error() {
        let inputs = FinalizeInputs {
            video: None,
            microphone: None,
            system_audio: None,
            output: Path::new("x"),
            trim_to: None,
        };

        assert!(matches!(
            final_mux_args(&inputs),
            Err(FfmpegError::InvalidInputs(_))
        ));
    }

    #[test]
    fn progress_clocks_are_parsed() {
        assert_eq!(
            parse_clock("frame=1 time=00:00:02.00 bitrate=N/A"),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            parse_clock("a time=00:01:30.50 b time=01:02:03.25 x"),
            Some(Duration::from_secs_f64(3723.25))
        );
        assert_eq!(parse_clock("no clock here"), None);
        assert_eq!(parse_clock("time=N/A"), None);
    }

    // --- the process wrapper, with a stand-in program that never reads its input ---

    fn sleeper() -> Command {
        #[cfg(windows)]
        {
            let mut c = Command::new("powershell.exe");
            c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            no_console_window(&mut c);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        }
    }

    #[test]
    fn a_full_queue_discards_items_and_counts_them_instead_of_blocking() {
        let encoder = StreamEncoder::spawn_command("stand-in", sleeper(), 2).unwrap();

        let mut outcomes = Vec::new();
        for _ in 0..200 {
            // 1 MB each: far more than the pipe and the 2-item queue can hold while nothing reads.
            outcomes.push(encoder.try_send(vec![0u8; 1 << 20]));
        }

        assert!(outcomes.contains(&SendOutcome::Queued));
        assert!(
            outcomes.contains(&SendOutcome::Dropped),
            "the queue must overflow, not grow without bound"
        );
        assert!(encoder.dropped() > 100);
        encoder.abort();
    }

    #[test]
    fn a_process_that_does_not_finish_is_killed_after_the_timeout() {
        let encoder = StreamEncoder::spawn_command("stand-in", sleeper(), 2).unwrap();
        let started = Instant::now();

        let error = encoder.finish(Duration::from_millis(300)).unwrap_err();

        assert!(matches!(error, FfmpegError::Timeout(_)), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "must not wait for the 30 s sleep"
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_a_spawn_error() {
        let result = StreamEncoder::spawn_command(
            "missing",
            Command::new("this-program-does-not-exist-xyz"),
            2,
        );

        assert!(matches!(result, Err(FfmpegError::Spawn(..))));
    }
}
