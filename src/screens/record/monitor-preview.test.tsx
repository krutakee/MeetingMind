import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { MonitorPreview } from '@/screens/record/monitor-preview'
import { installFakeBackend } from '@/test/fake-backend'

// TEST DOUBLES: the Tauri bridge is replaced for every test (src/test/setup.ts, src/test/fake-backend.ts).

// Windows device names, written with every backslash doubled in a JavaScript string.
const DISPLAY2 = '\\\\.\\DISPLAY2'
const DISPLAY9 = '\\\\.\\DISPLAY9'

/** What get_preview_frame answers: sequence, width, height (little-endian), then RGBA. */
function frameBytes(sequence: number, width: number, height: number): ArrayBuffer {
  const bytes = new ArrayBuffer(16 + width * height * 4)
  const header = new DataView(bytes)
  header.setBigUint64(0, BigInt(sequence), true)
  header.setUint32(8, width, true)
  header.setUint32(12, height, true)
  return bytes
}

describe('MonitorPreview', () => {
  it('starts the preview of the chosen monitor and shows what arrives', async () => {
    const backend = installFakeBackend({ previewFrame: frameBytes(1, 4, 2) })

    render(<MonitorPreview monitorId={DISPLAY2} />)

    const picture = await screen.findByRole('img', { name: /Live preview/ })
    await waitFor(() => expect(picture).toHaveAttribute('width', '4'))
    expect(picture).toHaveAttribute('height', '2')
    expect(backend.argsOf('start_preview')).toEqual([{ monitorId: DISPLAY2 }])
    expect(backend.argsOf('get_preview_frame')[0]).toEqual({ since: 0 })
  })

  it('asks only for pictures newer than the last one it showed', async () => {
    const backend = installFakeBackend({ previewFrame: frameBytes(7, 2, 2) })

    render(<MonitorPreview monitorId={null} />)

    await waitFor(() => expect(backend.argsOf('get_preview_frame').length).toBeGreaterThan(1), { timeout: 3000 })
    expect(backend.argsOf('get_preview_frame')[1]).toEqual({ since: 7 })
  })

  it('stops the preview when it goes away', async () => {
    const backend = installFakeBackend()
    const { unmount } = render(<MonitorPreview monitorId={null} />)
    await waitFor(() => expect(backend.argsOf('start_preview')).toHaveLength(1))

    unmount()

    expect(backend.argsOf('stop_preview')).toHaveLength(1)
  })

  it('restarts for another monitor', async () => {
    const backend = installFakeBackend()
    const { rerender } = render(<MonitorPreview monitorId="A" />)
    await waitFor(() => expect(backend.argsOf('start_preview')).toHaveLength(1))

    rerender(<MonitorPreview monitorId="B" />)

    await waitFor(() => expect(backend.argsOf('start_preview')).toHaveLength(2))
    expect(backend.argsOf('start_preview')[1]).toEqual({ monitorId: 'B' })
    expect(backend.argsOf('stop_preview')).toHaveLength(1)
  })

  it('waits quietly for the first picture', async () => {
    installFakeBackend()

    render(<MonitorPreview monitorId={null} />)

    expect(await screen.findByText('Waiting for the preview…')).toBeInTheDocument()
  })

  it('says why there is no preview', async () => {
    const backend = installFakeBackend()
    const { invoke } = await import('@tauri-apps/api/core')
    const normal = vi.mocked(invoke).getMockImplementation()
    vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'start_preview') throw { kind: 'unavailable', message: `the monitor ${DISPLAY9} was not found` }
      return normal ? normal(command, args as never) : undefined
    })

    render(<MonitorPreview monitorId={DISPLAY9} />)

    expect(await screen.findByRole('alert')).toHaveTextContent('was not found')
    expect(screen.getByText('No preview available.')).toBeInTheDocument()
    expect(backend.argsOf('get_preview_frame')).toEqual([])
  })
})
