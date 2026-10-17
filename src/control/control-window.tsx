import { useEffect, useState } from 'react'
import { Pause, Play, Square } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { LevelMeter } from '@/components/ui/level-meter'
import { formatDuration } from '@/lib/format'
import { errorMessage, ipc } from '@/lib/ipc'
import { applyTheme } from '@/lib/theme'
import { RecorderStatusProvider, useRecorderStatus } from '@/state/recorder-status'

/**
 * The small window that stays on top while recording (its own web page in its own window). It is hidden from the
 * recording by the backend. It shows the time and the two levels and has Pause and Stop; dragging its background
 * moves it.
 */
export function ControlWindow() {
  // The window has no app state of its own: it only needs the colours the user chose.
  useEffect(() => {
    let cancelled = false
    ipc
      .getSettings()
      .then((settings) => {
        if (!cancelled) applyTheme(settings.theme)
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [])

  return (
    <RecorderStatusProvider>
      <ControlPanel />
    </RecorderStatusProvider>
  )
}

function ControlPanel() {
  const { status, refresh } = useRecorderStatus()
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  if (!status || (status.phase !== 'recording' && status.phase !== 'paused')) {
    return (
      <div className="flex h-full items-center justify-center bg-background p-3 text-sm text-muted-foreground" role="status">
        {status?.phase === 'finalizing' ? 'Saving…' : 'Not recording'}
      </div>
    )
  }

  const paused = status.phase === 'paused'

  async function run(action: () => Promise<unknown>) {
    setBusy(true)
    setError(null)
    try {
      await action()
    } catch (failure) {
      setError(errorMessage(failure))
    } finally {
      setBusy(false)
      void refresh()
    }
  }

  return (
    <div
      data-tauri-drag-region
      className="flex h-full select-none flex-col justify-between gap-1 border-2 border-border bg-background px-3 py-2"
    >
      <div data-tauri-drag-region className="flex items-center gap-3">
        <span
          aria-hidden="true"
          className={`size-3 shrink-0 rounded-full ${paused ? 'bg-muted-foreground' : 'animate-pulse bg-destructive'}`}
        />
        <span className="w-16 text-sm font-medium">{paused ? 'Paused' : 'Recording'}</span>
        <span role="timer" aria-label="Recorded time" className="flex-1 text-lg font-semibold tabular-nums">
          {formatDuration(status.elapsedMs)}
        </span>
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          aria-label={paused ? 'Resume recording' : 'Pause recording'}
          onClick={() => void run(() => (paused ? ipc.resumeRecording() : ipc.pauseRecording()))}
        >
          {paused ? <Play aria-hidden="true" className="size-4" /> : <Pause aria-hidden="true" className="size-4" />}
        </Button>
        <Button size="sm" disabled={busy} aria-label="Stop and save the recording" onClick={() => void run(() => ipc.stopRecording())}>
          <Square aria-hidden="true" className="size-4" />
        </Button>
      </div>
      {error ? (
        <p role="alert" className="truncate text-xs text-destructive">
          {error}
        </p>
      ) : (
        <div className="grid grid-cols-2 gap-3">
          <LevelMeter label="Mic" level={status.microphoneLevel} />
          <LevelMeter label="System" level={status.systemAudioLevel} />
        </div>
      )}
    </div>
  )
}
