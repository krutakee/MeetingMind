import type { Settings } from '@/lib/ipc'

export type SaveSettings = (change: Partial<Settings>) => Promise<void>

/**
 * Saves settings one change at a time. The backend stores the full settings object, so each save starts from the
 * last values the backend confirmed and applies the change on top. Two quick changes therefore never overwrite one
 * another, and a rejected save does not block the ones queued behind it.
 *
 * @param send   stores the full settings; resolves with the values as stored (the backend may normalize them)
 * @param onSaved called with the stored values after every successful save
 */
export function createSettingsSaver(
  initial: Settings,
  send: (settings: Settings) => Promise<Settings>,
  onSaved: (saved: Settings) => void,
): SaveSettings {
  let lastSaved = initial
  let queue: Promise<unknown> = Promise.resolve()

  return (change) => {
    const run = queue.then(async () => {
      const saved = await send({ ...lastSaved, ...change })
      lastSaved = saved
      onSaved(saved)
    })
    queue = run.catch(() => undefined)
    return run
  }
}
