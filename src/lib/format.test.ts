import { describe, expect, it } from 'vitest'
import { formatBytes, formatDuration, formatStartedAt, levelPercent } from '@/lib/format'

describe('formatDuration', () => {
  it('shows minutes and seconds under an hour', () => {
    expect(formatDuration(0)).toBe('0:00')
    expect(formatDuration(5_400)).toBe('0:05')
    expect(formatDuration(65_000)).toBe('1:05')
    expect(formatDuration(59 * 60_000 + 59_999)).toBe('59:59')
  })

  it('adds hours from one hour on and keeps two digits for minutes and seconds', () => {
    expect(formatDuration(3_600_000)).toBe('1:00:00')
    expect(formatDuration(2 * 3_600_000 + 7 * 60_000 + 9_000)).toBe('2:07:09')
  })

  it('never goes negative', () => {
    expect(formatDuration(-500)).toBe('0:00')
  })
})

describe('formatBytes', () => {
  it('uses whole numbers for small sizes and one decimal from megabytes on', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1536)).toBe('2 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
    expect(formatBytes(3.3 * 1024 ** 3)).toBe('3.3 GB')
    expect(formatBytes(2 * 1024 ** 4)).toBe('2.0 TB')
  })

  it('does not invent units beyond terabytes', () => {
    expect(formatBytes(5000 * 1024 ** 4)).toBe('5000.0 TB')
  })
})

describe('formatStartedAt', () => {
  it('formats a real time and leaves unreadable text alone', () => {
    expect(formatStartedAt('2026-10-16T09:05:00Z')).toMatch(/2026/)
    expect(formatStartedAt('not a date')).toBe('not a date')
  })
})

describe('levelPercent', () => {
  it('rounds and keeps the level between 0 and 100', () => {
    expect(levelPercent(0.456)).toBe(46)
    expect(levelPercent(-1)).toBe(0)
    expect(levelPercent(3)).toBe(100)
    expect(levelPercent(Number.NaN)).toBe(0)
  })
})
