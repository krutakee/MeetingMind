import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Modal } from '@/components/ui/modal'
import { formatDuration, formatStartedAt } from '@/lib/format'
import { errorMessage, ipc, type Unfinished } from '@/lib/ipc'

/**
 * Shown at launch when a recording was not finished (the app or the computer stopped while recording). Each one can
 * be recovered into a playable file or deleted; "Decide later" keeps them for the next launch.
 */
export function RecoveryPrompt({ onRecovered }: { onRecovered: () => void }) {
  const [unfinished, setUnfinished] = useState<Unfinished[]>([])
  const [open, setOpen] = useState(false)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    ipc
      .listUnfinished()
      .then((rows) => {
        if (!cancelled && rows.length > 0) {
          setUnfinished(rows)
          setOpen(true)
        }
      })
      .catch(() => undefined) // Not being able to look is not worth a dialog; the Record screen still works.
    return () => {
      cancelled = true
    }
  }, [])

  // One action runs at a time (the buttons are disabled meanwhile), so the list in this render is still current.
  function done(id: number) {
    const rest = unfinished.filter((row) => row.id !== id)
    setUnfinished(rest)
    if (rest.length === 0) setOpen(false)
  }

  async function recover(row: Unfinished) {
    setBusyId(row.id)
    setProblem(null)
    setMessage(null)
    try {
      const recovered = await ipc.recoverRecording(row.id)
      setMessage(`Recovered "${row.title}" (${formatDuration(recovered.durationMs)}). It is in the recent recordings.`)
      done(row.id)
      onRecovered()
    } catch (failure) {
      setProblem(`"${row.title}" could not be recovered: ${errorMessage(failure)}`)
    } finally {
      setBusyId(null)
    }
  }

  async function discard(row: Unfinished) {
    setBusyId(row.id)
    setProblem(null)
    try {
      await ipc.discardRecording(row.id)
      setConfirmDeleteId(null)
      done(row.id)
    } catch (failure) {
      setProblem(`"${row.title}" could not be deleted: ${errorMessage(failure)}`)
    } finally {
      setBusyId(null)
    }
  }

  return (
    <>
      {message && (
        <p role="status" className="rounded-md border-2 border-border bg-card p-4 text-sm">
          {message}
        </p>
      )}
      <Modal open={open} title="Unfinished recording found" onClose={() => setOpen(false)}>
        <p className="text-sm">
          MeetingMind stopped before {unfinished.length === 1 ? 'this recording was' : 'these recordings were'} finished.
          Everything recorded up to that point is still on disk and can be recovered.
        </p>
        <ul className="space-y-3">
          {unfinished.map((row) => (
            <li key={row.id} className="rounded-md border border-border p-3">
              <p className="font-medium">{row.title}</p>
              <p className="text-sm text-muted-foreground">Started {formatStartedAt(row.startedAt)}</p>
              <div className="mt-2 flex flex-wrap gap-2">
                <Button size="sm" disabled={busyId !== null} onClick={() => void recover(row)}>
                  {busyId === row.id && confirmDeleteId !== row.id ? 'Recovering…' : 'Recover'}
                </Button>
                {confirmDeleteId === row.id ? (
                  <>
                    <Button size="sm" variant="outline" disabled={busyId !== null} onClick={() => void discard(row)}>
                      Yes, delete it
                    </Button>
                    <Button size="sm" variant="outline" disabled={busyId !== null} onClick={() => setConfirmDeleteId(null)}>
                      Keep it
                    </Button>
                  </>
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busyId !== null}
                    aria-label={`Delete ${row.title}`}
                    onClick={() => setConfirmDeleteId(row.id)}
                  >
                    Delete…
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
        {problem && (
          <p role="alert" className="text-sm text-destructive">
            {problem}
          </p>
        )}
        <div className="flex justify-end">
          <Button variant="outline" onClick={() => setOpen(false)} disabled={busyId !== null}>
            Decide later
          </Button>
        </div>
      </Modal>
    </>
  )
}
