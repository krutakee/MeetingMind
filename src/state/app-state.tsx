import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { errorMessage, ipc, type AppInfo, type Settings } from '@/lib/ipc'
import { createSettingsSaver, type SaveSettings } from '@/lib/settings-saver'
import { applyTheme } from '@/lib/theme'
import { LoadingScreen, StartupFailedScreen } from '@/screens/startup-screens'

type Load =
  | { status: 'loading' }
  | { status: 'failed'; message: string }
  | { status: 'ready'; info: AppInfo; settings: Settings | null; save: SaveSettings }

interface AppStateValue {
  info: AppInfo
  /** null only when the app started with an error (see info.startupError). */
  settings: Settings | null
  /** Changes some settings and saves them all. Rejects with the backend's message if a value is not accepted. */
  saveSettings: SaveSettings
}

const AppStateContext = createContext<AppStateValue | null>(null)

export function useAppState(): AppStateValue {
  const value = useContext(AppStateContext)
  if (!value) throw new Error('useAppState must be used inside <AppProvider>')
  return value
}

const settingsUnavailable: SaveSettings = () => Promise.reject(new Error('Settings are not available.'))

/** Loads app info and settings once at startup, applies the saved theme, and holds both for the screens. */
export function AppProvider({ children }: { children: ReactNode }) {
  const [load, setLoad] = useState<Load>({ status: 'loading' })

  useEffect(() => {
    let cancelled = false
    async function start() {
      try {
        const info = await ipc.getAppInfo()
        const settings = info.startupError ? null : await ipc.getSettings()
        if (cancelled) return
        const save = settings
          ? createSettingsSaver(settings, ipc.updateSettings, (saved) =>
              setLoad((current) => (current.status === 'ready' ? { ...current, settings: saved } : current)),
            )
          : settingsUnavailable
        setLoad({ status: 'ready', info, settings, save })
      } catch (error) {
        if (!cancelled) setLoad({ status: 'failed', message: errorMessage(error) })
      }
    }
    void start()
    return () => {
      cancelled = true
    }
  }, [])

  const theme = load.status === 'ready' ? (load.settings?.theme ?? 'system') : 'system'
  useEffect(() => applyTheme(theme), [theme])

  const value = useMemo<AppStateValue | null>(
    () =>
      load.status === 'ready' ? { info: load.info, settings: load.settings, saveSettings: load.save } : null,
    [load],
  )

  if (!value) {
    if (load.status === 'failed') {
      return <StartupFailedScreen title="MeetingMind could not reach its backend" message={load.message} />
    }
    return <LoadingScreen />
  }
  return <AppStateContext.Provider value={value}>{children}</AppStateContext.Provider>
}
