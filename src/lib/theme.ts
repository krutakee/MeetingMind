import type { Theme } from '@/lib/ipc'

export const THEME_LABELS: Record<Theme, string> = {
  system: 'System default',
  light: 'Light',
  dark: 'Dark',
}

/** Which palette is showing, given the saved choice and the OS preference. */
export function resolveTheme(theme: Theme, systemPrefersDark: boolean): 'light' | 'dark' {
  if (theme === 'system') return systemPrefersDark ? 'dark' : 'light'
  return theme
}

/**
 * Applies the saved choice to the page. "System" removes the attribute so the stylesheet follows the OS
 * (prefers-color-scheme); "Light" and "Dark" set it, which overrides the OS.
 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement) {
  if (theme === 'system') {
    root.removeAttribute('data-theme')
  } else {
    root.setAttribute('data-theme', theme)
  }
}
