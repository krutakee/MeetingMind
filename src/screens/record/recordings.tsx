import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Notice } from '@/components/ui/notice'
import { formatDuration, formatStartedAt } from '@/lib/format'
import { errorMessage, ipc, type Finished, type RecentRecording } from '@/lib/ipc'
import type { PlayableRecording } from '@/screens/record/player-dialog'

/** How many recordings the Record screen lists. */
const RECENT_LIMIT = 10

/** Shown after a recording was stopped and saved: what was saved, with the two things to do next. */
export function FinishedBanner({
  finished,
  onPlay,
  onDismiss,
}: {
  finished: Finished
  onPlay: (recording: PlayableRecording) => void
  onDismiss: () => void
}) {
  const [problem, setProblem] = useState<string | null>(null)

  async function openFolder() {
    try {
      await ipc.openFolder(finished.folder)
    } catch (failure) {
      setProblem(errorMessage(failure))
    }
  }

  return (
    <Notice tone="success">
      <p className="font-medium">Recording saved</p>
      <p className="mt-1 text-muted-foreground">
        {finished.title} · {formatDuration(finished.durationMs)}
      </p>
      {problem && <p className="mt-1 text-destructive">{problem}</p>}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button size="sm" onClick={() => onPlay(finished)}>
          Play
        </Button>
        <Button size="sm" variant="outline" onClick={() => void openFolder()}>
          Open folder
        </Button>
        <Button size="sm" variant="outline" onClick={onDismiss}>
          Dismiss
        </Button>
      </div>
    </Notice>
  )
}

/** The latest saved recordings, each with Play and Open folder. Refreshes when `refreshKey` changes. */
export function RecentRecordings({
  refreshKey,
  onPlay,
}: {
  refreshKey: unknown
  onPlay: (recording: PlayableRecording) => void
}) {
  const [recordings, setRecordings] = useState<RecentRecording[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    ipc
      .listRecentRecordings(RECENT_LIMIT)
      .then((rows) => {
        if (!cancelled) {
          setRecordings(rows)
          setError(null)
        }
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(errorMessage(failure))
      })
    return () => {
      cancelled = true
    }
  }, [refreshKey])

  async function openFolder(folder: string) {
    try {
      await ipc.openFolder(folder)
    } catch (failure) {
      setError(errorMessage(failure))
    }
  }

  return (
    <section aria-labelledby="recent-heading" className="space-y-3">
      <h2 id="recent-heading" className="text-lg font-semibold">
        Recent recordings
      </h2>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {recordings && recordings.length === 0 && !error && (
        <p className="text-sm text-muted-foreground">Nothing recorded yet. Your recordings will be listed here.</p>
      )}
      {recordings && recordings.length > 0 && (
        <ul className="divide-y divide-border rounded-md border border-border">
          {recordings.map((recording) => (
            <li key={recording.id} className="flex flex-wrap items-center gap-3 p-3">
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium">{recording.title}</p>
                <p className="text-sm text-muted-foreground">
                  {formatStartedAt(recording.startedAt)}
                  {recording.durationMs !== null && ` · ${formatDuration(recording.durationMs)}`}
                  {!recording.file && ' · the file is no longer there'}
                </p>
              </div>
              <div className="flex gap-2">
                <Button
                  size="sm"
                  disabled={!recording.file}
                  aria-label={`Play ${recording.title}`}
                  onClick={() => {
                    if (recording.file) {
                      onPlay({
                        title: recording.title,
                        file: recording.file,
                        folder: recording.folder,
                        hasVideo: recording.hasVideo,
                      })
                    }
                  }}
                >
                  Play
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  aria-label={`Open the folder of ${recording.title}`}
                  onClick={() => void openFolder(recording.folder)}
                >
                  Open folder
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
