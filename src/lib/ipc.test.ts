import { invoke } from '@tauri-apps/api/core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { errorMessage, ipc, type Settings } from '@/lib/ipc'

// TEST DOUBLE: the Tauri bridge is replaced so no backend is needed. These tests pin the command names and
// argument shapes that src-tauri/src/lib.rs registers; if either side is renamed, a test here must change too.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

describe('ipc commands', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockResolvedValue(undefined)
  })

  it('calls the backend commands by their registered names', async () => {
    await ipc.getAppInfo()
    await ipc.getSettings()
    await ipc.getCapabilities()

    expect(vi.mocked(invoke).mock.calls).toEqual([['get_app_info'], ['get_settings'], ['get_capabilities']])
  })

  it('sends settings as the "settings" argument', async () => {
    const settings: Settings = { storageFolder: null, theme: 'dark', consentReminder: false }

    await ipc.updateSettings(settings)

    expect(invoke).toHaveBeenCalledWith('update_settings', { settings })
  })
})

describe('errorMessage', () => {
  it('reads the message of a backend error object', () => {
    expect(errorMessage({ kind: 'invalidSetting', message: "invalid setting 'storageFolder'" })).toBe(
      "invalid setting 'storageFolder'",
    )
  })

  it('reads strings and Error instances', () => {
    expect(errorMessage('plain text')).toBe('plain text')
    expect(errorMessage(new Error('boom'))).toBe('boom')
  })

  it('falls back to a generic message for anything else', () => {
    expect(errorMessage(undefined)).toBe('Something went wrong.')
    expect(errorMessage({ message: 42 })).toBe('Something went wrong.')
  })
})
