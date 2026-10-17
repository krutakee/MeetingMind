/**
 * Typed wrappers for the backend commands in src-tauri/src/commands.rs.
 * Field names are camelCase on this side and in the JSON; the Rust structs rename them with serde.
 * Keep these types in step with the Rust types when either side changes.
 */
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export type Theme = 'system' | 'light' | 'dark'

/** Global shortcuts as accelerator text, for example `Ctrl+Shift+S`. */
export interface Hotkeys {
  startStop: string
  pauseResume: string
}

export interface Settings {
  /** null means "use the default folder". */
  storageFolder: string | null
  theme: Theme
  consentReminder: boolean
  hotkeys: Hotkeys
}

export interface AppInfo {
  productName: string
  version: string
  dataDir: string
  logDir: string
  dbPath: string
  defaultStorageFolder: string
  schemaVersion: number | null
  /** Set when the database could not be opened. The app then shows this instead of its normal screens. */
  startupError: string | null
  startupWarnings: string[]
}

export type CapabilityId =
  | 'ram'
  | 'cpu_cores'
  | 'system_audio_capture'
  | 'capture_exclusion'
  | 'gpu_backend'
  | 'video_encoders'

export type CapabilityStatus = 'detected' | 'unavailable' | 'unknown' | 'notYetImplemented'

export interface Capability {
  id: CapabilityId
  status: CapabilityStatus
  /** Only present when status is "detected". */
  value: string | null
  detail: string
}

export interface CapabilityReport {
  platform: string
  capabilities: Capability[]
}

/** What the backend sends when a command fails. */
export interface BackendError {
  kind: string
  message: string
}

// ---------------------------------------------------------------------------------------------------------------
// Recording (src-tauri/src/recorder.rs, recording/setup.rs, commands.rs)

export type Quality = 'p720' | 'p1080' | 'native'
export type RecordingMode = 'screenAudio' | 'audioOnly'

/** Everything chosen before a recording starts. */
export interface RecordingSetup {
  /** Empty means "use a default title". */
  title: string
  mode: RecordingMode
  /** null is the primary monitor. */
  monitorId: string | null
  microphone: boolean
  /** null is the default microphone. */
  microphoneId: string | null
  systemAudio: boolean
  quality: Quality
  fps: number
}

export interface MonitorInfo {
  id: string
  name: string
  width: number
  height: number
  isPrimary: boolean
}

export interface AudioDeviceInfo {
  id: string
  name: string
  isDefault: boolean
}

/** Each list can fail on its own; the problem says why. */
export interface DeviceLists {
  monitors: MonitorInfo[]
  monitorsProblem: string | null
  microphones: AudioDeviceInfo[]
  microphonesProblem: string | null
}

export interface Estimate {
  /** The most disk space one hour can take. */
  maxBytesPerHour: number
  videoWidth: number | null
  videoHeight: number | null
  freeBytes: number | null
  storageFolder: string
}

export type Phase = 'idle' | 'starting' | 'recording' | 'paused' | 'finalizing'

export type WarningKind =
  | 'diskLow'
  | 'diskCritical'
  | 'sourceUnavailable'
  | 'keepAwakeFailed'
  | 'ownWindowVisible'
  | 'sourceLost'
  | 'sourceRestored'
  | 'encoderFailed'

export interface RecordingWarning {
  id: number
  kind: WarningKind
  stream: 'video' | 'microphone' | 'systemAudio' | null
  message: string
  atMediaMs: number
}

/** A recording that ended and was saved. */
export interface Finished {
  meetingId: number | null
  title: string
  folder: string
  /** recording.mp4 or recording.m4a */
  file: string
  durationMs: number
  hasVideo: boolean
}

/** Sent to every window about four times a second while a recording is active. */
export interface RecorderStatus {
  phase: Phase
  title: string | null
  mode: RecordingMode | null
  elapsedMs: number
  /** Peak level of the latest audio, 0 to 1. */
  microphoneLevel: number
  systemAudioLevel: number
  warnings: RecordingWarning[]
  autoStop: 'diskFull' | null
  finished: Finished | null
  error: string | null
}

export interface Preset {
  id: number
  name: string
  setup: RecordingSetup
}

export interface RecentRecording {
  id: number
  title: string
  startedAt: string
  durationMs: number | null
  folder: string
  /** null when the file is no longer there. */
  file: string | null
  hasVideo: boolean
}

export interface Unfinished {
  id: number
  title: string
  startedAt: string
  folder: string
}

export interface RecoveredRecording {
  meetingId: number
  folder: string
  file: string
  durationMs: number
}

/** The event the recorder sends its status on. */
export const STATUS_EVENT = 'recorder-status'

export const ipc = {
  getAppInfo: () => invoke<AppInfo>('get_app_info'),
  getSettings: () => invoke<Settings>('get_settings'),
  updateSettings: (settings: Settings) => invoke<Settings>('update_settings', { settings }),
  getCapabilities: () => invoke<CapabilityReport>('get_capabilities'),

  listDevices: () => invoke<DeviceLists>('list_devices'),
  estimateRecording: (setup: RecordingSetup) => invoke<Estimate>('estimate_recording', { setup }),
  startPreview: (monitorId: string | null) => invoke<void>('start_preview', { monitorId }),
  stopPreview: () => invoke<void>('stop_preview'),
  getPreviewFrame: (since: number) => invoke<ArrayBuffer>('get_preview_frame', { since }),

  recordingStatus: () => invoke<RecorderStatus>('recording_status'),
  /** consentGiven: the user has just confirmed the consent reminder. */
  startRecording: (setup: RecordingSetup, consentGiven: boolean) =>
    invoke<void>('start_recording', { setup, consentGiven }),
  pauseRecording: () => invoke<void>('pause_recording'),
  resumeRecording: () => invoke<void>('resume_recording'),
  stopRecording: () => invoke<Finished>('stop_recording'),
  cancelRecording: () => invoke<void>('cancel_recording'),
  dismissRecordingResult: () => invoke<void>('dismiss_recording_result'),

  listPresets: () => invoke<Preset[]>('list_presets'),
  savePreset: (name: string, setup: RecordingSetup) => invoke<Preset>('save_preset', { name, setup }),
  deletePreset: (id: number) => invoke<void>('delete_preset', { id }),
  getLastSetup: () => invoke<RecordingSetup | null>('get_last_setup'),

  listRecentRecordings: (limit: number) => invoke<RecentRecording[]>('list_recent_recordings', { limit }),
  openFolder: (path: string) => invoke<void>('open_folder', { path }),

  listUnfinished: () => invoke<Unfinished[]>('list_unfinished'),
  recoverRecording: (id: number) => invoke<RecoveredRecording>('recover_recording', { id }),
  discardRecording: (id: number) => invoke<void>('discard_recording', { id }),
}

/** Calls `handler` with every status the recorder sends. Resolves with the function that stops listening. */
export function listenToStatus(handler: (status: RecorderStatus) => void): Promise<() => void> {
  return listen<RecorderStatus>(STATUS_EVENT, (event) => handler(event.payload))
}

/** One live-preview picture: RGBA pixels. */
export interface PreviewFrame {
  sequence: number
  width: number
  height: number
  rgba: Uint8ClampedArray<ArrayBuffer>
}

const PREVIEW_HEADER_BYTES = 16

/**
 * Reads what `get_preview_frame` returns: 8 bytes sequence, 4 width, 4 height (little-endian), then RGBA pixels.
 * An empty or damaged answer means "nothing newer" and gives null.
 */
export function parsePreviewFrame(bytes: ArrayBuffer): PreviewFrame | null {
  if (bytes.byteLength <= PREVIEW_HEADER_BYTES) return null
  const header = new DataView(bytes)
  const sequence = Number(header.getBigUint64(0, true))
  const width = header.getUint32(8, true)
  const height = header.getUint32(12, true)
  if (width === 0 || height === 0 || bytes.byteLength !== PREVIEW_HEADER_BYTES + width * height * 4) return null
  return { sequence, width, height, rgba: new Uint8ClampedArray(bytes, PREVIEW_HEADER_BYTES) }
}

/** A readable message from whatever a failed command (or any other failure) threw. */
export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const { message } = error as { message: unknown }
    if (typeof message === 'string') return message
  }
  return 'Something went wrong.'
}

/** The `kind` of a backend error (`consentRequired`, `busy`, ...), or null if it has none. */
export function errorKind(error: unknown): string | null {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    const { kind } = error as { kind: unknown }
    if (typeof kind === 'string') return kind
  }
  return null
}
