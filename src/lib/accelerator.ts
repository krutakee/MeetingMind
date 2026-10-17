/**
 * Turns a key press into the shortcut text the backend registers (`Ctrl+Shift+S`).
 *
 * Only keys that work the same on every keyboard layout are accepted (letters, digits, function keys, a few named
 * keys), and a shortcut must include Ctrl, Alt or the Windows/Command key, so it cannot hijack plain typing.
 */

export type AcceleratorResult =
  | { kind: 'ready'; accelerator: string }
  /** Only modifier keys are down so far: keep waiting. */
  | { kind: 'waiting' }
  | { kind: 'refused'; reason: string }

const NAMED_KEYS: Record<string, string> = {
  Space: 'Space',
  Enter: 'Enter',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  Insert: 'Insert',
  Delete: 'Delete',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
}

const MODIFIER_CODES = /^(Control|Shift|Alt|Meta|OS)(Left|Right)?$/

/** The key part of the shortcut for a physical key (`KeyS`, `Digit1`, `F5`, `ArrowUp`), or null if not allowed. */
function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code)
  if (letter) return letter[1]
  const digit = /^Digit([0-9])$/.exec(code)
  if (digit) return digit[1]
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code
  return NAMED_KEYS[code] ?? null
}

export function acceleratorFromKeyEvent(
  event: Pick<KeyboardEvent, 'code' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>,
): AcceleratorResult {
  if (MODIFIER_CODES.test(event.code)) return { kind: 'waiting' }
  const key = keyName(event.code)
  if (!key) return { kind: 'refused', reason: 'That key cannot be used in a shortcut. Use a letter, digit or F-key.' }
  if (!event.ctrlKey && !event.altKey && !event.metaKey) {
    return { kind: 'refused', reason: 'Hold Ctrl, Alt or the Windows key as well, so the shortcut does not get in the way of typing.' }
  }
  const parts: string[] = []
  if (event.ctrlKey) parts.push('Ctrl')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  if (event.metaKey) parts.push('Super')
  parts.push(key)
  return { kind: 'ready', accelerator: parts.join('+') }
}
