import { describe, expect, it, vi } from 'vitest'
import type { Settings } from '@/lib/ipc'
import { createSettingsSaver } from '@/lib/settings-saver'

const INITIAL: Settings = { storageFolder: null, theme: 'system', consentReminder: true }

/** A promise the test resolves by hand, to control when a "save" finishes. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('createSettingsSaver', () => {
  it('sends the change merged into the current settings and reports what was stored', async () => {
    const send = vi.fn(async (settings: Settings) => settings)
    const onSaved = vi.fn()
    const save = createSettingsSaver(INITIAL, send, onSaved)

    await save({ theme: 'dark' })

    expect(send).toHaveBeenCalledWith({ ...INITIAL, theme: 'dark' })
    expect(onSaved).toHaveBeenCalledWith({ ...INITIAL, theme: 'dark' })
  })

  it('uses the values the backend stored, not the ones it was sent', async () => {
    const send = vi.fn(async (settings: Settings) => ({ ...settings, storageFolder: 'C:\\trimmed' }))
    const save = createSettingsSaver(INITIAL, send, () => {})

    await save({ storageFolder: '  C:\\trimmed  ' })
    await save({ theme: 'light' })

    expect(send).toHaveBeenLastCalledWith({ storageFolder: 'C:\\trimmed', theme: 'light', consentReminder: true })
  })

  it('does not start a second save until the first has finished, and keeps both changes', async () => {
    const first = deferred<Settings>()
    const send = vi
      .fn<(settings: Settings) => Promise<Settings>>()
      .mockReturnValueOnce(first.promise)
      .mockImplementation(async (settings) => settings)
    const save = createSettingsSaver(INITIAL, send, () => {})

    const a = save({ consentReminder: false })
    const b = save({ theme: 'dark' })
    await Promise.resolve()
    expect(send).toHaveBeenCalledTimes(1)

    first.resolve({ ...INITIAL, consentReminder: false })
    await Promise.all([a, b])

    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenLastCalledWith({ storageFolder: null, theme: 'dark', consentReminder: false })
  })

  it('rejects a failed save, leaves the settings unchanged, and still runs the next save', async () => {
    const send = vi
      .fn<(settings: Settings) => Promise<Settings>>()
      .mockRejectedValueOnce({ kind: 'invalidSetting', message: 'nope' })
      .mockImplementation(async (settings) => settings)
    const onSaved = vi.fn()
    const save = createSettingsSaver(INITIAL, send, onSaved)

    const failing = save({ theme: 'dark' })
    const next = save({ consentReminder: false })

    await expect(failing).rejects.toMatchObject({ message: 'nope' })
    await next
    // The failed theme change is not carried into the next save.
    expect(send).toHaveBeenLastCalledWith({ storageFolder: null, theme: 'system', consentReminder: false })
    expect(onSaved).toHaveBeenCalledTimes(1)
  })
})
