/** Elapsed or recorded time as `m:ss`, or `h:mm:ss` from one hour on. */
export function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1000))
  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60
  const two = (n: number) => String(n).padStart(2, '0')
  return hours > 0 ? `${hours}:${two(minutes)}:${two(seconds)}` : `${minutes}:${two(seconds)}`
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

/** A size in bytes the way Windows shows it: units of 1024, one decimal from megabytes on. */
export function formatBytes(bytes: number): string {
  let value = Math.max(0, bytes)
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  return unit < 2 ? `${Math.round(value)} ${UNITS[unit]}` : `${value.toFixed(1)} ${UNITS[unit]}`
}

/** When a recording started, in the reader's own language and time zone. Falls back to the raw text if unreadable. */
export function formatStartedAt(iso: string): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

/** A level between 0 and 1 as a whole percentage, kept in range. */
export function levelPercent(level: number): number {
  if (!Number.isFinite(level)) return 0
  return Math.round(Math.min(1, Math.max(0, level)) * 100)
}
