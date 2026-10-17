import { describe, expect, it } from 'vitest'
import { acceleratorFromKeyEvent } from '@/lib/accelerator'

const keys = (code: string, modifiers: Partial<Record<'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey', boolean>> = {}) => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...modifiers,
})

describe('acceleratorFromKeyEvent', () => {
  it('writes a letter with its modifiers in a fixed order', () => {
    expect(acceleratorFromKeyEvent(keys('KeyS', { ctrlKey: true, shiftKey: true }))).toEqual({
      kind: 'ready',
      accelerator: 'Ctrl+Shift+S',
    })
    expect(acceleratorFromKeyEvent(keys('KeyR', { shiftKey: true, altKey: true, ctrlKey: true }))).toEqual({
      kind: 'ready',
      accelerator: 'Ctrl+Alt+Shift+R',
    })
  })

  it('accepts digits, function keys and named keys', () => {
    expect(acceleratorFromKeyEvent(keys('Digit1', { altKey: true }))).toEqual({ kind: 'ready', accelerator: 'Alt+1' })
    expect(acceleratorFromKeyEvent(keys('F9', { ctrlKey: true }))).toEqual({ kind: 'ready', accelerator: 'Ctrl+F9' })
    expect(acceleratorFromKeyEvent(keys('ArrowUp', { ctrlKey: true, altKey: true }))).toEqual({
      kind: 'ready',
      accelerator: 'Ctrl+Alt+Up',
    })
    expect(acceleratorFromKeyEvent(keys('Space', { metaKey: true }))).toEqual({ kind: 'ready', accelerator: 'Super+Space' })
  })

  it('waits while only modifier keys are held', () => {
    for (const code of ['ControlLeft', 'ShiftRight', 'AltLeft', 'MetaLeft', 'OSRight']) {
      expect(acceleratorFromKeyEvent(keys(code, { ctrlKey: true }))).toEqual({ kind: 'waiting' })
    }
  })

  it('refuses a shortcut with no Ctrl, Alt or Windows key, and says why', () => {
    const result = acceleratorFromKeyEvent(keys('KeyS', { shiftKey: true }))

    expect(result.kind).toBe('refused')
    expect(result).toMatchObject({ reason: expect.stringContaining('Ctrl, Alt') })
    expect(acceleratorFromKeyEvent(keys('KeyS')).kind).toBe('refused')
  })

  it('refuses keys that differ between keyboard layouts', () => {
    const result = acceleratorFromKeyEvent(keys('Semicolon', { ctrlKey: true }))

    expect(result.kind).toBe('refused')
    expect(result).toMatchObject({ reason: expect.stringContaining('cannot be used') })
  })

  it('does not accept F25 or beyond', () => {
    expect(acceleratorFromKeyEvent(keys('F24', { ctrlKey: true })).kind).toBe('ready')
    expect(acceleratorFromKeyEvent(keys('F25', { ctrlKey: true })).kind).toBe('refused')
  })
})
