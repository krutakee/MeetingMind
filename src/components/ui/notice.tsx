import type { ReactNode } from 'react'
import { AlertTriangle, CheckCircle2, Info, XCircle, type LucideIcon } from 'lucide-react'
import { cn } from '@/lib/utils'

type Tone = 'info' | 'warning' | 'error' | 'success'

const TONES: Record<Tone, { icon: LucideIcon; label: string; box: string }> = {
  info: { icon: Info, label: 'Note', box: 'border-border bg-card' },
  warning: { icon: AlertTriangle, label: 'Warning', box: 'border-border bg-muted' },
  error: { icon: XCircle, label: 'Error', box: 'border-destructive text-destructive' },
  success: { icon: CheckCircle2, label: 'Done', box: 'border-border bg-card' },
}

/**
 * A message box. The tone is never carried by colour alone: each has its own icon and a word for screen readers.
 * Errors and warnings are announced at once (role "alert"); the others politely (role "status").
 */
export function Notice({
  tone = 'info',
  children,
  className,
}: {
  tone?: Tone
  children: ReactNode
  className?: string
}) {
  const { icon: Icon, label, box } = TONES[tone]
  return (
    <div
      role={tone === 'error' || tone === 'warning' ? 'alert' : 'status'}
      className={cn('flex gap-3 rounded-md border-2 p-4 text-sm', box, className)}
    >
      <Icon aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
      <div className="min-w-0 flex-1">
        <span className="sr-only">{label}: </span>
        {children}
      </div>
    </div>
  )
}
