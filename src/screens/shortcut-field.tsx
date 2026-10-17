import { useState, type KeyboardEvent } from 'react'
import { Button } from '@/components/ui/button'
import { acceleratorFromKeyEvent } from '@/lib/accelerator'

/**
 * Shows a shortcut and changes it by pressing the new combination. Escape (or leaving the button) cancels. A
 * combination that is not allowed is explained and the field keeps waiting.
 */
export function ShortcutField({
  label,
  value,
  onChange,
}: {
  label: string
  value: string
  /** Saves the new shortcut. A refusal by the backend is shown by the owner. */
  onChange: (accelerator: string) => Promise<void>
}) {
  const [capturing, setCapturing] = useState(false)
  const [hint, setHint] = useState<string | null>(null)

  function stop() {
    setCapturing(false)
    setHint(null)
  }

  async function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (!capturing) return
    event.preventDefault()
    event.stopPropagation()
    if (event.key === 'Escape' && !event.ctrlKey && !event.altKey && !event.metaKey) {
      stop()
      return
    }
    const result = acceleratorFromKeyEvent(event.nativeEvent)
    if (result.kind === 'waiting') return
    if (result.kind === 'refused') {
      setHint(result.reason)
      return
    }
    stop()
    await onChange(result.accelerator)
  }

  return (
    <div className="flex flex-wrap items-center gap-3">
      <span className="w-56 text-sm">{label}</span>
      <Button
        variant="outline"
        className="min-w-48 font-mono"
        aria-label={`${label}: ${value}. Press to change.`}
        onClick={() => setCapturing(true)}
        onBlur={stop}
        onKeyDown={(event) => void onKeyDown(event)}
      >
        {capturing ? 'Press the new keys…' : value}
      </Button>
      {capturing && <span className="text-sm text-muted-foreground">Esc cancels.</span>}
      {hint && (
        <span role="alert" className="text-sm text-destructive">
          {hint}
        </span>
      )}
    </div>
  )
}
