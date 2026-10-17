import { invoke } from '@tauri-apps/api/core'
import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ControlWindow } from '@/control/control-window'
import { DEFAULT_SETTINGS, IDLE_STATUS, RECORDING_STATUS, installFakeBackend } from '@/test/fake-backend'

// TEST DOUBLES: the Tauri bridge is replaced for every test (src/test/setup.ts, src/test/fake-backend.ts).

describe('Control window', () => {
  it('shows the time and the levels while recording', async () => {
    installFakeBackend({ status: RECORDING_STATUS })

    render(<ControlWindow />)

    expect(await screen.findByRole('timer')).toHaveTextContent('1:05')
    expect(screen.getByText('Recording')).toBeInTheDocument()
    expect(screen.getByRole('meter', { name: 'Mic level' })).toHaveAttribute('aria-valuenow', '40')
    expect(screen.getByRole('meter', { name: 'System level' })).toHaveAttribute('aria-valuenow', '20')
  })

  it('pauses and resumes from its own buttons', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    render(<ControlWindow />)

    await userEvent.click(await screen.findByRole('button', { name: 'Pause recording' }))

    expect(await screen.findByText('Paused')).toBeInTheDocument()
    expect(backend.argsOf('pause_recording')).toHaveLength(1)

    await userEvent.click(screen.getByRole('button', { name: 'Resume recording' }))

    expect(await screen.findByText('Recording')).toBeInTheDocument()
    expect(backend.argsOf('resume_recording')).toHaveLength(1)
  })

  it('stops and saves from its own button', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    render(<ControlWindow />)

    await userEvent.click(await screen.findByRole('button', { name: 'Stop and save the recording' }))

    await waitFor(() => expect(backend.argsOf('stop_recording')).toHaveLength(1))
    expect(await screen.findByText('Not recording')).toBeInTheDocument()
  })

  it('follows the status the backend pushes', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    render(<ControlWindow />)
    await screen.findByRole('timer')

    act(() => backend.emitStatus({ ...RECORDING_STATUS, elapsedMs: 125_000, systemAudioLevel: 0.75 }))

    expect(screen.getByRole('timer')).toHaveTextContent('2:05')
    expect(screen.getByRole('meter', { name: 'System level' })).toHaveAttribute('aria-valuenow', '75')
  })

  it('says it is saving while the recording is being finished, and not recording when idle', async () => {
    const backend = installFakeBackend({ status: { ...RECORDING_STATUS, phase: 'finalizing' } })
    render(<ControlWindow />)

    expect(await screen.findByText('Saving…')).toBeInTheDocument()

    act(() => backend.emitStatus(IDLE_STATUS))

    expect(screen.getByText('Not recording')).toBeInTheDocument()
  })

  it('shows why a button did not work', async () => {
    installFakeBackend({ status: RECORDING_STATUS })
    render(<ControlWindow />)
    await screen.findByRole('timer')
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      if (command === 'pause_recording') throw { kind: 'session', message: 'a recording cannot go from Paused to Paused' }
      return RECORDING_STATUS
    })

    await userEvent.click(screen.getByRole('button', { name: 'Pause recording' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('cannot go from Paused to Paused')
  })

  it('uses the colours the user chose', async () => {
    installFakeBackend({ status: RECORDING_STATUS, settings: { ...DEFAULT_SETTINGS, theme: 'dark' } })

    render(<ControlWindow />)

    await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'dark'))
  })

  it('can be dragged by its background (the window has no title bar)', async () => {
    installFakeBackend({ status: RECORDING_STATUS })

    render(<ControlWindow />)

    const timer = await screen.findByRole('timer')
    expect(timer.closest('[data-tauri-drag-region]')).not.toBeNull()
  })
})
