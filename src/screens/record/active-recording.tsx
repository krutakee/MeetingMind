import { useState } from 'react'
import { Pause, Play, Square } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { LevelMeter } from '@/components/ui/level-meter'
import { ConfirmDialog } from '@/components/ui/modal'
import { Notice } from '@/components/ui/notice'
import { formatDuration } from '@/lib/format'
import { errorMessage, ipc, type RecorderStatus, type WarningKind } from '@/lib/ipc'
import { useAppState } from '@/state/app-state'
import { useRecorderStatus } from '@/state/recorder-status'

type NoticeTone = 'warning' | 'error' | 'info'

/** How serious a warning from the recording is. A lost source is a warning: the recording carries on without it. */
export function toneForWarning(kind: WarningKind): NoticeTone {
  switch (kind) {
    case 'diskCritical':
    case 'encoderFailed':
      return 'error'
    case 'sourceRestored':
      return 'info'
    default:
      return 'warning'
  }
}

/** The screen while a recording is running or paused: time, levels, the controls, and anything gone wrong. */
export function ActiveRecording({ status }: { status: RecorderStatus }) {
  const { settings } = useAppState()
  const { refresh } = useRecorderStatus()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [confirmingCancel, setConfirmingCancel] = useState(false)
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
    <section aria-label="Recording in progress" className="space-y-6">
      <div className="flex items-center gap-3">
        <span
          aria-hidden="true"
          className={`size-3 rounded-full ${paused ? 'bg-muted-foreground' : 'animate-pulse bg-destructive'}`}
        />
        <p className="text-lg font-semibold">{paused ? 'Paused' : 'Recording'}</p>
        {status.title && <p className="truncate text-muted-foreground">{status.title}</p>}
      </div>

      <p role="timer" aria-label="Recorded time" className="text-6xl font-semibold tabular-nums">
        {formatDuration(status.elapsedMs)}
      </p>

      <div className="max-w-md space-y-3">
        <LevelMeter label="Microphone" level={status.microphoneLevel} />
        <LevelMeter label="System" level={status.systemAudioLevel} />
      </div>

      <div className="flex flex-wrap gap-3">
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => void run(() => (paused ? ipc.resumeRecording() : ipc.pauseRecording()))}
        >
          {paused ? <Play aria-hidden="true" className="size-4" /> : <Pause aria-hidden="true" className="size-4" />}
          {paused ? 'Resume' : 'Pause'}
        </Button>
        <Button disabled={busy} onClick={() => void run(() => ipc.stopRecording())}>
          <Square aria-hidden="true" className="size-4" />
          Stop and save
        </Button>
        <Button variant="outline" disabled={busy} onClick={() => setConfirmingCancel(true)}>
          Discard…
        </Button>
      </div>

      {error && <Notice tone="error">{error}</Notice>}
      {status.autoStop === 'diskFull' && (
        <Notice tone="error">The disk is almost full. The recording is being stopped and saved.</Notice>
      )}
      {status.warnings.map((warning) => (
        <Notice key={warning.id} tone={toneForWarning(warning.kind)}>
          {warning.message}
        </Notice>
      ))}

      <p className="text-sm text-muted-foreground">
        A small control window stays on top of your other programs. MeetingMind’s own windows are hidden from the
        recording. Shortcuts that work in any program: {settings?.hotkeys.startStop ?? 'Ctrl+Shift+S'} stops,{' '}
        {settings?.hotkeys.pauseResume ?? 'Ctrl+Shift+P'} pauses and resumes.
      </p>

      <ConfirmDialog
        open={confirmingCancel}
        title="Discard this recording?"
        confirmLabel="Discard recording"
        cancelLabel="Keep recording"
        busy={busy}
        onCancel={() => setConfirmingCancel(false)}
        onConfirm={() => {
          setConfirmingCancel(false)
          void run(() => ipc.cancelRecording())
        }}
      >
        <p>Everything recorded so far is deleted. This cannot be undone.</p>
      </ConfirmDialog>
    </section>
  )
}
