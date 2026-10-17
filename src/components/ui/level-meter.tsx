import { levelPercent } from '@/lib/format'

/** A horizontal level bar for the microphone or system audio. The value is also readable as text by assistive tech. */
export function LevelMeter({ label, level }: { label: string; level: number }) {
  const percent = levelPercent(level)
  return (
    <div className="flex items-center gap-3 text-sm">
      <span className="w-16 shrink-0 text-muted-foreground">{label}</span>
      <div
        role="meter"
        aria-label={`${label} level`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        className="h-2.5 min-w-0 flex-1 overflow-hidden rounded-full border border-border bg-muted"
      >
        <div className="h-full bg-primary transition-[width] duration-150" style={{ width: `${percent}%` }} />
      </div>
    </div>
  )
}
