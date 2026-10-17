import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

const CONTROL =
  'h-9 w-full rounded-md border border-border bg-background px-3 text-sm ' +
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-50'

/** The browser's own select, styled like the other controls: keyboard and screen reader support come for free. */
export function Select({ className, ...props }: ComponentProps<'select'>) {
  return <select className={cn(CONTROL, className)} {...props} />
}

export function TextInput({ className, type = 'text', ...props }: ComponentProps<'input'>) {
  return <input type={type} className={cn(CONTROL, className)} {...props} />
}
