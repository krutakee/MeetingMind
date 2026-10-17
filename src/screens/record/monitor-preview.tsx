import { useEffect, useRef, useState } from 'react'
import { errorMessage, ipc, parsePreviewFrame } from '@/lib/ipc'

/** How often a newer picture is asked for. The backend produces about two per second. */
const POLL_MS = 500

/**
 * A small live picture of the monitor that would be recorded: the same capture as the recording uses, so what is
 * shown is what would be saved. It runs only while this component is on screen and stops with it. Choosing another
 * monitor starts a fresh preview (the inner component is keyed by the monitor, so nothing of the old one is kept).
 */
export function MonitorPreview({ monitorId }: { monitorId: string | null }) {
  return <PreviewOfMonitor key={monitorId ?? 'main-monitor'} monitorId={monitorId} />
}

function PreviewOfMonitor({ monitorId }: { monitorId: string | null }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [hasPicture, setHasPicture] = useState(false)

  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    let lastSequence = 0

    async function poll() {
      try {
        const frame = parsePreviewFrame(await ipc.getPreviewFrame(lastSequence))
        if (cancelled) return
        if (frame) {
          lastSequence = frame.sequence
          const canvas = canvasRef.current
          if (canvas) {
            canvas.width = frame.width
            canvas.height = frame.height
            canvas.getContext('2d')?.putImageData(new ImageData(frame.rgba, frame.width, frame.height), 0, 0)
          }
          setHasPicture(true)
        }
      } catch (failure) {
        if (!cancelled) setProblem(errorMessage(failure))
      }
      if (!cancelled) timer = setTimeout(() => void poll(), POLL_MS)
    }

    ipc
      .startPreview(monitorId)
      .then(() => {
        if (!cancelled) void poll()
      })
      .catch((failure: unknown) => {
        if (!cancelled) setProblem(errorMessage(failure))
      })

    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
      void ipc.stopPreview().catch(() => undefined)
    }
  }, [monitorId])

  return (
    <figure className="space-y-2">
      <div className="flex aspect-video w-full max-w-md items-center justify-center overflow-hidden rounded-md border-2 border-border bg-muted">
        <canvas
          ref={canvasRef}
          role="img"
          aria-label="Live preview of the monitor that will be recorded"
          className={hasPicture ? 'size-full object-contain' : 'hidden'}
        />
        {!hasPicture && (
          <p className="p-4 text-center text-sm text-muted-foreground">
            {problem ? 'No preview available.' : 'Waiting for the preview…'}
          </p>
        )}
      </div>
      {problem && (
        <figcaption role="alert" className="text-sm text-destructive">
          The preview could not be shown: {problem}
        </figcaption>
      )}
    </figure>
  )
}
