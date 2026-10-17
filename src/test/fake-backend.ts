/**
 * TEST DOUBLE. Stands in for the Rust backend in frontend tests only; app code never imports this file.
 *
 * It answers the backend commands the way the real backend answers them (same JSON shapes, camelCase), keeps
 * settings and the recording state in memory, pushes status events like the real recorder does, and records every
 * call. It is NOT the real backend: it proves how the UI behaves, not that the Rust side is correct (the Rust tests
 * cover that).
 *
 * `@tauri-apps/api/core` and `@tauri-apps/api/event` are replaced for every test in src/test/setup.ts.
 */
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { vi } from 'vitest'
import {
  STATUS_EVENT,
  type AppInfo,
  type BackendError,
  type CapabilityReport,
  type DeviceLists,
  type Estimate,
  type Finished,
  type Preset,
  type RecentRecording,
  type RecorderStatus,
  type RecordingSetup,
  type Settings,
  type Unfinished,
} from '@/lib/ipc'

export const DEFAULT_INFO: AppInfo = {
  productName: 'MeetingMind',
  version: '0.1.0',
  dataDir: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop',
  logDir: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop\\logs',
  dbPath: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop\\meetingmind.db',
  defaultStorageFolder: 'C:\\Users\\test\\Documents\\MeetingMind\\Meetings',
  schemaVersion: 2,
  startupError: null,
  startupWarnings: [],
}

export const DEFAULT_SETTINGS: Settings = {
  storageFolder: null,
  theme: 'system',
  consentReminder: true,
  hotkeys: { startStop: 'Ctrl+Shift+S', pauseResume: 'Ctrl+Shift+P' },
}

export const DEFAULT_CAPABILITIES: CapabilityReport = {
  platform: 'windows',
  capabilities: [
    { id: 'ram', status: 'detected', value: '16.0 GiB', detail: 'Total installed memory.' },
    {
      id: 'cpu_cores',
      status: 'detected',
      value: '8 threads (6 cores)',
      detail: 'Processor threads available to MeetingMind.',
    },
    {
      id: 'system_audio_capture',
      status: 'notYetImplemented',
      value: null,
      detail: 'Whether system audio can be captured is not checked yet (planned for Phase 2).',
    },
    {
      id: 'capture_exclusion',
      status: 'notYetImplemented',
      value: null,
      detail: 'Not checked yet.',
    },
    { id: 'gpu_backend', status: 'notYetImplemented', value: null, detail: 'Not detected yet.' },
    { id: 'video_encoders', status: 'notYetImplemented', value: null, detail: 'Not detected yet.' },
  ],
}

export const DEFAULT_DEVICES: DeviceLists = {
  monitors: [
    { id: '\\\\.\\DISPLAY2', name: 'HP 27xw', width: 1920, height: 1080, isPrimary: true },
    { id: '\\\\.\\DISPLAY3', name: 'DELL U2412M', width: 1920, height: 1200, isPrimary: false },
  ],
  monitorsProblem: null,
  microphones: [
    { id: 'mic-built-in', name: 'Microphone Array (Built-in)', isDefault: true },
    { id: 'mic-usb', name: 'USB Headset', isDefault: false },
  ],
  microphonesProblem: null,
}

export const DEFAULT_ESTIMATE: Estimate = {
  maxBytesPerHour: 3.3 * 1024 ** 3,
  videoWidth: 1920,
  videoHeight: 1080,
  freeBytes: 200 * 1024 ** 3,
  storageFolder: 'C:\\Users\\test\\Documents\\MeetingMind\\Meetings',
  storageSyncedBy: null,
}

export const IDLE_STATUS: RecorderStatus = {
  phase: 'idle',
  title: null,
  mode: null,
  elapsedMs: 0,
  microphoneLevel: 0,
  systemAudioLevel: 0,
  warnings: [],
  autoStop: null,
  finished: null,
  error: null,
}

export const RECORDING_STATUS: RecorderStatus = {
  ...IDLE_STATUS,
  phase: 'recording',
  title: 'Weekly sync',
  mode: 'screenAudio',
  elapsedMs: 65_000,
  microphoneLevel: 0.4,
  systemAudioLevel: 0.2,
}

export const FINISHED: Finished = {
  meetingId: 7,
  title: 'Weekly sync',
  folder: 'C:\\Users\\test\\Documents\\MeetingMind\\Meetings\\2026-10-16_0905_Weekly sync',
  file: 'C:\\Users\\test\\Documents\\MeetingMind\\Meetings\\2026-10-16_0905_Weekly sync\\recording.mp4',
  durationMs: 1_800_000,
  hasVideo: true,
}

export interface FakeBackendOptions {
  info?: Partial<AppInfo>
  settings?: Settings
  capabilities?: CapabilityReport
  /** Makes every update_settings call fail with this error, as the real backend does for invalid values. */
  updateError?: BackendError
  devices?: DeviceLists
  estimate?: Estimate
  status?: RecorderStatus
  presets?: Preset[]
  lastSetup?: RecordingSetup | null
  recent?: RecentRecording[]
  unfinished?: Unfinished[]
  /** Makes start_recording fail with this error. */
  startError?: BackendError
  /** Makes recover_recording fail with this error. */
  recoverError?: BackendError
  /** The cloud storage program storage_sync_provider reports for every folder (default: none). */
  storageSyncedBy?: string | null
  /** The bytes get_preview_frame answers with (default: nothing yet). */
  previewFrame?: ArrayBuffer
}

export interface FakeBackend {
  settings: Settings
  /** The recorder status the backend would send now. */
  status: RecorderStatus
  presets: Preset[]
  unfinished: Unfinished[]
  /** Every command received, in order. */
  calls: { command: string; args: unknown }[]
  /** Arguments of every update_settings call. */
  updates: () => Settings[]
  /** Arguments of every call of one command. */
  argsOf: (command: string) => unknown[]
  /** Changes the status and pushes it to every listener, as the real recorder does about four times a second. */
  emitStatus: (status: RecorderStatus) => void
}

type Listener = (event: { payload: unknown }) => void

export function installFakeBackend(options: FakeBackendOptions = {}): FakeBackend {
  const listeners = new Set<Listener>()
  const backend: FakeBackend = {
    settings: options.settings ?? DEFAULT_SETTINGS,
    status: options.status ?? IDLE_STATUS,
    presets: options.presets ?? [],
    unfinished: options.unfinished ?? [],
    calls: [],
    updates: () => backend.argsOf('update_settings').map((args) => (args as { settings: Settings }).settings),
    argsOf: (command) => backend.calls.filter((call) => call.command === command).map((call) => call.args),
    emitStatus: (status) => {
      backend.status = status
      for (const listener of listeners) listener({ payload: status })
    },
  }
  let nextPresetId = 100

  vi.mocked(listen).mockImplementation((async (event: string, handler: Listener) => {
    if (event !== STATUS_EVENT) return () => {}
    listeners.add(handler)
    return () => listeners.delete(handler)
  }) as unknown as typeof listen)

  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    backend.calls.push({ command, args })
    switch (command) {
      case 'get_app_info':
        return { ...DEFAULT_INFO, ...options.info }
      case 'get_settings':
        return backend.settings
      case 'update_settings': {
        if (options.updateError) throw options.updateError
        backend.settings = (args as { settings: Settings }).settings
        return backend.settings
      }
      case 'get_capabilities':
        return options.capabilities ?? DEFAULT_CAPABILITIES

      case 'list_devices':
        return options.devices ?? DEFAULT_DEVICES
      case 'estimate_recording':
        return options.estimate ?? DEFAULT_ESTIMATE
      case 'storage_sync_provider':
        return options.storageSyncedBy ?? null
      case 'start_preview':
      case 'stop_preview':
        return undefined
      case 'get_preview_frame':
        return options.previewFrame ?? new ArrayBuffer(0)

      case 'recording_status':
        return backend.status
      case 'start_recording': {
        if (options.startError) throw options.startError
        const { setup } = args as { setup: RecordingSetup }
        backend.emitStatus({ ...RECORDING_STATUS, title: setup.title || 'Meeting', mode: setup.mode, elapsedMs: 0 })
        return undefined
      }
      case 'pause_recording':
        backend.emitStatus({ ...backend.status, phase: 'paused' })
        return undefined
      case 'resume_recording':
        backend.emitStatus({ ...backend.status, phase: 'recording' })
        return undefined
      case 'stop_recording':
        backend.emitStatus({ ...IDLE_STATUS, finished: FINISHED })
        return FINISHED
      case 'cancel_recording':
        backend.emitStatus(IDLE_STATUS)
        return undefined
      case 'dismiss_recording_result':
        backend.status = { ...backend.status, finished: null, error: null }
        return undefined

      case 'list_presets':
        return backend.presets
      case 'save_preset': {
        const { name, setup } = args as { name: string; setup: RecordingSetup }
        const preset: Preset = { id: nextPresetId++, name, setup: { ...setup, title: '' } }
        backend.presets = [...backend.presets.filter((p) => p.name.toLowerCase() !== name.toLowerCase()), preset]
        return preset
      }
      case 'delete_preset':
        backend.presets = backend.presets.filter((p) => p.id !== (args as { id: number }).id)
        return undefined
      case 'get_last_setup':
        return options.lastSetup ?? null

      case 'list_recent_recordings':
        return options.recent ?? []
      case 'open_folder':
        return undefined

      case 'list_unfinished':
        return backend.unfinished
      case 'recover_recording': {
        if (options.recoverError) throw options.recoverError
        const { id } = args as { id: number }
        backend.unfinished = backend.unfinished.filter((row) => row.id !== id)
        return { meetingId: id, folder: 'C:\\Meetings\\recovered', file: 'C:\\Meetings\\recovered\\recording.mp4', durationMs: 120_000 }
      }
      case 'discard_recording':
        backend.unfinished = backend.unfinished.filter((row) => row.id !== (args as { id: number }).id)
        return undefined

      default:
        throw new Error(`fake backend: unknown command ${command}`)
    }
  })

  return backend
}
