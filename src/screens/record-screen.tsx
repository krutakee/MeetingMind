import { useState } from 'react'
import { Notice } from '@/components/ui/notice'
import { errorMessage, ipc } from '@/lib/ipc'
import { ActiveRecording } from '@/screens/record/active-recording'
import { PlayerDialog, type PlayableRecording } from '@/screens/record/player-dialog'
import { FinishedBanner, RecentRecordings } from '@/screens/record/recordings'
import { RecoveryPrompt } from '@/screens/record/recovery-prompt'
import { SetupPanel } from '@/screens/record/setup-panel'
import { useRecorderStatus } from '@/state/recorder-status'

export function RecordScreen() {
  const { status, error: statusError, refresh } = useRecorderStatus()
  const [playing, setPlaying] = useState<PlayableRecording | null>(null)
  const [recoveredCount, setRecoveredCount] = useState(0)
  const [dismissError, setDismissError] = useState<string | null>(null)

  async function dismiss() {
    setDismissError(null)
    try {
      await ipc.dismissRecordingResult()
      await refresh()
    } catch (failure) {
      setDismissError(errorMessage(failure))
    }
  }

  return (
    <div className="max-w-5xl space-y-8">
      <h1 className="text-2xl font-semibold">Record</h1>

      {!status && statusError && <Notice tone="error">The recorder did not answer: {statusError}</Notice>}
      {!status && !statusError && <p role="status" className="text-muted-foreground">Checking the recorder…</p>}

      {status?.phase === 'starting' && <p role="status">Starting the recording…</p>}
      {status?.phase === 'finalizing' && (
        <p role="status" className="text-lg">
          Saving the recording… Please keep MeetingMind open. This can take a moment after a long recording.
        </p>
      )}
      {(status?.phase === 'recording' || status?.phase === 'paused') && <ActiveRecording status={status} />}

      {status?.phase === 'idle' && (
        <>
          <RecoveryPrompt onRecovered={() => setRecoveredCount((count) => count + 1)} />
          {status.finished && <FinishedBanner finished={status.finished} onPlay={setPlaying} onDismiss={() => void dismiss()} />}
          {status.error && <Notice tone="error">The recording could not be finished into one file: {status.error}</Notice>}
          {dismissError && <Notice tone="error">{dismissError}</Notice>}
          <SetupPanel />
          <RecentRecordings refreshKey={`${status.finished?.file ?? ''}|${recoveredCount}`} onPlay={setPlaying} />
        </>
      )}

      <PlayerDialog recording={playing} onClose={() => setPlaying(null)} />
    </div>
  )
}
