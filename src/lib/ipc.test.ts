import { invoke } from '@tauri-apps/api/core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { errorKind, errorMessage, ipc, parsePreviewFrame, type RecordingSetup, type Settings } from '@/lib/ipc'

// TEST DOUBLE: the Tauri bridge is replaced for every test (src/test/setup.ts) so no backend is needed. These tests
// pin the command names and argument shapes that src-tauri/src/lib.rs registers; if either side is renamed, a test
// here must change too.

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
    const settings: Settings = {
      storageFolder: null,
      theme: 'dark',
      consentReminder: false,
      hotkeys: { startStop: 'Ctrl+Alt+R', pauseResume: 'Ctrl+Alt+P' },
    }

    await ipc.updateSettings(settings)

    expect(invoke).toHaveBeenCalledWith('update_settings', { settings })
  })
})

describe('recording commands', () => {
  const setup: RecordingSetup = {
    title: 'Weekly sync',
    mode: 'screenAudio',
    monitorId: null,
    microphone: true,
    microphoneId: null,
    systemAudio: true,
    quality: 'p1080',
    fps: 15,
  }

  beforeEach(() => {
    vi.mocked(invoke).mockResolvedValue(undefined)
  })

  it('calls the recording commands by their registered names with camelCase arguments', async () => {
    await ipc.listDevices()
    await ipc.estimateRecording(setup)
    await ipc.startPreview('A')
    await ipc.stopPreview()
    await ipc.getPreviewFrame(3)
    await ipc.recordingStatus()
    await ipc.startRecording(setup, true)
    await ipc.pauseRecording()
    await ipc.resumeRecording()
    await ipc.stopRecording()
    await ipc.cancelRecording()
    await ipc.dismissRecordingResult()

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ['list_devices'],
      ['estimate_recording', { setup }],
      ['start_preview', { monitorId: 'A' }],
      ['stop_preview'],
      ['get_preview_frame', { since: 3 }],
      ['recording_status'],
      ['start_recording', { setup, consentGiven: true }],
      ['pause_recording'],
      ['resume_recording'],
      ['stop_recording'],
      ['cancel_recording'],
      ['dismiss_recording_result'],
    ])
  })

  it('calls the preset, recent-recording and recovery commands by their registered names', async () => {
    await ipc.listPresets()
    await ipc.savePreset('Standup', setup)
    await ipc.deletePreset(4)
    await ipc.getLastSetup()
    await ipc.listRecentRecordings(10)
    await ipc.openFolder('C:\\Meetings\\x')
    await ipc.listUnfinished()
    await ipc.recoverRecording(5)
    await ipc.discardRecording(6)

    expect(vi.mocked(invoke).mock.calls).toEqual([
      ['list_presets'],
      ['save_preset', { name: 'Standup', setup }],
      ['delete_preset', { id: 4 }],
      ['get_last_setup'],
      ['list_recent_recordings', { limit: 10 }],
      ['open_folder', { path: 'C:\\Meetings\\x' }],
      ['list_unfinished'],
      ['recover_recording', { id: 5 }],
      ['discard_recording', { id: 6 }],
    ])
  })
})

describe('storage folder check', () => {
  it('asks the backend which cloud storage program uploads a folder', async () => {
    vi.mocked(invoke).mockResolvedValue('OneDrive')

    const provider = await ipc.storageSyncProvider('C:\\Users\\a\\OneDrive\\Meetings')

    expect(provider).toBe('OneDrive')
    expect(invoke).toHaveBeenCalledWith('storage_sync_provider', { path: 'C:\\Users\\a\\OneDrive\\Meetings' })
  })
})

describe('parsePreviewFrame', () => {
  function bytes(sequence: number, width: number, height: number, extra = 0): ArrayBuffer {
    const buffer = new ArrayBuffer(16 + width * height * 4 + extra)
    const header = new DataView(buffer)
    header.setBigUint64(0, BigInt(sequence), true)
    header.setUint32(8, width, true)
    header.setUint32(12, height, true)
    new Uint8Array(buffer, 16).fill(9)
    return buffer
  }

  it('reads the header and the pixels', () => {
    const frame = parsePreviewFrame(bytes(42, 3, 2))

    expect(frame).not.toBeNull()
    expect(frame).toMatchObject({ sequence: 42, width: 3, height: 2 })
    expect(frame?.rgba.length).toBe(3 * 2 * 4)
    expect(frame?.rgba[0]).toBe(9)
  })

  it('treats an empty answer as "nothing newer"', () => {
    expect(parsePreviewFrame(new ArrayBuffer(0))).toBeNull()
    expect(parsePreviewFrame(new ArrayBuffer(16))).toBeNull()
  })

  it('refuses a picture whose size does not match its header', () => {
    expect(parsePreviewFrame(bytes(1, 3, 2, 4))).toBeNull()
    expect(parsePreviewFrame(bytes(1, 0, 2))).toBeNull()
  })
})

describe('errorKind', () => {
  it('reads the kind of a backend error and gives null for anything else', () => {
    expect(errorKind({ kind: 'consentRequired', message: 'x' })).toBe('consentRequired')
    expect(errorKind('text')).toBeNull()
    expect(errorKind({ message: 'no kind' })).toBeNull()
    expect(errorKind(null)).toBeNull()
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
