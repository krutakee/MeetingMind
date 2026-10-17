import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach, vi } from 'vitest'

// TEST DOUBLES: the Tauri bridge is replaced for every test (see src/test/fake-backend.ts, which fills in the answers).
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  // Same shape as the real one on Windows: files are served by the asset protocol.
  convertFileSrc: (path: string) => `http://asset.localhost/${encodeURIComponent(path)}`,
}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-theme')
})

// TEST ENVIRONMENT POLYFILLS. jsdom lacks a few browser features the app uses. The real WebView2 / WKWebView engines
// provide all of them; these stubs only keep the components from crashing under test.

// Used by the Radix switch and radio controls.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = ResizeObserverStub

// <dialog>: opening and closing only toggles the `open` attribute (no top layer, no focus trap here).
if (!HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute('open', '')
  }
}
if (!HTMLDialogElement.prototype.close) {
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute('open')
  }
}

// <canvas>: there is no drawing surface, so nothing is painted. Tests check what the component asks for instead.
HTMLCanvasElement.prototype.getContext = (() => null) as unknown as typeof HTMLCanvasElement.prototype.getContext

// <video> and <audio>: nothing is played.
HTMLMediaElement.prototype.play = () => Promise.resolve()
HTMLMediaElement.prototype.pause = () => {}
HTMLMediaElement.prototype.load = () => {}

// ImageData: the preview builds one per picture.
if (typeof globalThis.ImageData === 'undefined') {
  class ImageDataStub {
    constructor(
      public data: Uint8ClampedArray,
      public width: number,
      public height: number,
    ) {}
  }
  globalThis.ImageData = ImageDataStub as unknown as typeof ImageData
}
