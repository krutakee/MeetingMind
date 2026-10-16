import { describe, expect, it } from 'vitest'
import { applyTheme, resolveTheme } from '@/lib/theme'

describe('resolveTheme', () => {
  it('follows the OS when the choice is "system"', () => {
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
  })

  it('ignores the OS when the user picked light or dark', () => {
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })
})

describe('applyTheme', () => {
  it('sets data-theme for light and dark', () => {
    const root = document.createElement('div')

    applyTheme('dark', root)
    expect(root.getAttribute('data-theme')).toBe('dark')

    applyTheme('light', root)
    expect(root.getAttribute('data-theme')).toBe('light')
  })

  it('removes data-theme for "system" so the stylesheet follows the OS', () => {
    const root = document.createElement('div')
    root.setAttribute('data-theme', 'dark')

    applyTheme('system', root)

    expect(root.hasAttribute('data-theme')).toBe(false)
  })
})
