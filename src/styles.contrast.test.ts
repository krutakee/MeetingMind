import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, expect, it } from 'vitest'

/** Reads the real src/index.css, so the contrast claims in its comment are checked, not just written down. */
const css = readFileSync(path.resolve(import.meta.dirname, 'index.css'), 'utf8')

function block(pattern: RegExp): Record<string, string> {
  const match = pattern.exec(css)
  if (!match) throw new Error(`index.css has no block matching ${pattern}`)
  const vars: Record<string, string> = {}
  for (const [, name, value] of match[1].matchAll(/--([a-z-]+):\s*(#[0-9a-fA-F]{6});/g)) {
    vars[name] = value
  }
  return vars
}

const light = block(/:root\s*\{([^}]*)\}/)
const darkFromOs = block(/:root:not\(\[data-theme='light'\]\)\s*\{([^}]*)\}/)
const darkChosen = block(/:root\[data-theme='dark'\]\s*\{([^}]*)\}/)

function luminance(hex: string): number {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
}

function contrast(a: string, b: string): number {
  const [lighter, darker] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (lighter + 0.05) / (darker + 0.05)
}

// [foreground, background, minimum ratio]: 4.5 for text (WCAG AA), 3 for meaningful non-text shapes.
const PAIRS: [string, string, number][] = [
  ['foreground', 'background', 4.5],
  ['foreground', 'card', 4.5],
  ['foreground', 'muted', 4.5],
  ['muted-foreground', 'background', 4.5],
  ['muted-foreground', 'card', 4.5],
  ['muted-foreground', 'muted', 4.5],
  ['primary-foreground', 'primary', 4.5],
  ['destructive', 'background', 4.5],
  ['border', 'background', 3],
  ['border', 'card', 3],
  ['primary', 'background', 3],
  ['ring', 'background', 3],
]

describe.each([
  ['light', light],
  ['dark', darkChosen],
])('%s palette', (_name, palette) => {
  it.each(PAIRS)('%s on %s is at least %s:1', (foreground, background, minimum) => {
    expect(contrast(palette[foreground], palette[background])).toBeGreaterThanOrEqual(minimum)
  })
})

describe('dark palette', () => {
  it('is identical whether it comes from the OS preference or the user’s choice', () => {
    expect(darkFromOs).toEqual(darkChosen)
  })
})
