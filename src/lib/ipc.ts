/**
 * Typed wrappers for the backend commands in src-tauri/src/commands.rs.
 * Field names are camelCase on this side and in the JSON; the Rust structs rename them with serde.
 * Keep these types in step with the Rust types when either side changes.
 */
import { invoke } from '@tauri-apps/api/core'

export type Theme = 'system' | 'light' | 'dark'

export interface Settings {
  /** null means "use the default folder". */
  storageFolder: string | null
  theme: Theme
  consentReminder: boolean
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

export const ipc = {
  getAppInfo: () => invoke<AppInfo>('get_app_info'),
  getSettings: () => invoke<Settings>('get_settings'),
  updateSettings: (settings: Settings) => invoke<Settings>('update_settings', { settings }),
  getCapabilities: () => invoke<CapabilityReport>('get_capabilities'),
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
