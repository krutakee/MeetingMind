import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { errorMessage, ipc, listenToStatus, type RecorderStatus } from '@/lib/ipc'

interface RecorderStatusValue {
  /** null until the backend has answered the first time. */
  status: RecorderStatus | null
  /** Set if the backend could not tell the status at all. */
  error: string | null
  /** Asks the backend for the status now, instead of waiting for the next push. */
  refresh: () => Promise<void>
}

type Held = Omit<RecorderStatusValue, 'refresh'>

const RecorderStatusContext = createContext<RecorderStatusValue | null>(null)

export function useRecorderStatus(): RecorderStatusValue {
  const value = useContext(RecorderStatusContext)
  if (!value) throw new Error('useRecorderStatus must be used inside <RecorderStatusProvider>')
  return value
}

/**
 * Keeps the latest recorder status for every screen of one window. The backend pushes a new status about four times
 * a second while something is happening; the first one is asked for, so a window opened mid-recording is right at once.
 */
export function RecorderStatusProvider({ children }: { children: ReactNode }) {
  const [held, setHeld] = useState<Held>({ status: null, error: null })

  useEffect(() => {
    let cancelled = false
    let stopListening: (() => void) | undefined

    listenToStatus((status) => {
      if (!cancelled) setHeld({ status, error: null })
    })
      .then((stop) => {
        if (cancelled) stop()
        else stopListening = stop
      })
      .catch((failure: unknown) => {
        if (!cancelled) setHeld((current) => ({ ...current, error: errorMessage(failure) }))
      })

    ipc
      .recordingStatus()
      .then((status) => {
        // An update that arrived while asking is newer than the answer.
        if (!cancelled) setHeld((current) => (current.status ? current : { status, error: null }))
      })
      .catch((failure: unknown) => {
        if (!cancelled) setHeld((current) => (current.status ? current : { status: null, error: errorMessage(failure) }))
      })

    return () => {
      cancelled = true
      stopListening?.()
    }
  }, [])

  const refresh = useCallback(async () => {
    try {
      const status = await ipc.recordingStatus()
      setHeld({ status, error: null })
    } catch (failure) {
      setHeld((current) => ({ ...current, error: errorMessage(failure) }))
    }
  }, [])

  const value = useMemo(() => ({ ...held, refresh }), [held, refresh])
  return <RecorderStatusContext.Provider value={value}>{children}</RecorderStatusContext.Provider>
}
