import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-theme')
})

// TEST ENVIRONMENT POLYFILL: jsdom has no ResizeObserver, which the Radix switch and radio controls use.
// The real WebView2 / WKWebView engines provide it. This stub only keeps the controls from crashing in tests.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = ResizeObserverStub
