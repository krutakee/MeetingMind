/**
 * TEST DOUBLE. Stands in for the Rust backend in frontend tests only; app code never imports this file.
 *
 * It answers the four backend commands the way the real backend answers them (same JSON shapes, camelCase), keeps
 * settings in memory, and records every call. It is NOT the real backend: it proves how the UI behaves, not that
 * the Rust side is correct (the Rust tests cover that).
 *
 * The test file must mock the Tauri module first:  vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
 */
import { invoke } from '@tauri-apps/api/core'
import { vi } from 'vitest'
import type { AppInfo, BackendError, CapabilityReport, Settings } from '@/lib/ipc'

export const DEFAULT_INFO: AppInfo = {
  productName: 'MeetingMind',
  version: '0.1.0',
  dataDir: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop',
  logDir: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop\\logs',
  dbPath: 'C:\\Users\\test\\AppData\\Roaming\\com.meetingmind.desktop\\meetingmind.db',
  defaultStorageFolder: 'C:\\Users\\test\\Documents\\MeetingMind\\Meetings',
  schemaVersion: 1,
  startupError: null,
  startupWarnings: [],
}

export const DEFAULT_SETTINGS: Settings = {
  storageFolder: null,
  theme: 'system',
  consentReminder: true,
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

export interface FakeBackendOptions {
  info?: Partial<AppInfo>
  settings?: Settings
  capabilities?: CapabilityReport
  /** Makes every update_settings call fail with this error, as the real backend does for invalid values. */
  updateError?: BackendError
}

export interface FakeBackend {
  settings: Settings
  /** Every command received, in order. */
  calls: { command: string; args: unknown }[]
  /** Arguments of every update_settings call. */
  updates: () => Settings[]
}

export function installFakeBackend(options: FakeBackendOptions = {}): FakeBackend {
  const backend: FakeBackend = {
    settings: options.settings ?? DEFAULT_SETTINGS,
    calls: [],
    updates: () =>
      backend.calls
        .filter((call) => call.command === 'update_settings')
        .map((call) => (call.args as { settings: Settings }).settings),
  }

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
      default:
        throw new Error(`fake backend: unknown command ${command}`)
    }
  })

  return backend
}
