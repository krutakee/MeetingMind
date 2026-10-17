import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { DEFAULT_INFO, DEFAULT_SETTINGS, installFakeBackend } from '@/test/fake-backend'
import { renderApp } from '@/test/render-app'

// TEST DOUBLES: the Tauri bridge is replaced for every test (src/test/setup.ts, src/test/fake-backend.ts); the native
// folder dialog is replaced here.
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

async function openSettings() {
  renderApp('/settings')
  await screen.findByRole('heading', { name: 'Settings' })
}

describe('Settings screen', () => {
  it('shows the saved values and the default storage folder', async () => {
    installFakeBackend()

    await openSettings()

    expect(screen.getByRole('radio', { name: 'System default' })).toBeChecked()
    expect(screen.getByRole('switch', { name: /consent/i })).toBeChecked()
    expect(screen.getByTestId('storage-folder')).toHaveTextContent(DEFAULT_INFO.defaultStorageFolder)
    expect(screen.getByText('This is the default folder.', { exact: false })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Use default folder' })).toBeDisabled()
  })

  it('lists what was detected and says plainly what is not implemented yet', async () => {
    installFakeBackend()

    await openSettings()

    const table = await screen.findByRole('table')
    const ram = within(table).getByRole('row', { name: /Memory \(RAM\)/ })
    expect(ram).toHaveTextContent('Detected: 16.0 GiB')
    const audio = within(table).getByRole('row', { name: /System audio capture/ })
    expect(audio).toHaveTextContent('Not yet implemented')
    expect(within(table).queryByText(/^Available$/)).not.toBeInTheDocument()
  })

  it('shows version, schema and folders under About', async () => {
    installFakeBackend()

    await openSettings()

    const about = screen.getByRole('region', { name: 'About' })
    expect(within(about).getByText('0.1.0')).toBeInTheDocument()
    expect(within(about).getByText(DEFAULT_INFO.logDir)).toBeInTheDocument()
  })

  it('saves the consent reminder switch and keeps the other values', async () => {
    const backend = installFakeBackend()
    await openSettings()

    await userEvent.click(screen.getByRole('switch', { name: /consent/i }))

    await waitFor(() => expect(screen.getByRole('switch', { name: /consent/i })).not.toBeChecked())
    expect(backend.updates()).toEqual([{ ...DEFAULT_SETTINGS, consentReminder: false }])
  })

  it('applies a chosen theme to the page immediately', async () => {
    const backend = installFakeBackend()
    await openSettings()

    await userEvent.click(screen.getByRole('radio', { name: 'Dark' }))

    await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'dark'))
    expect(backend.settings.theme).toBe('dark')

    await userEvent.click(screen.getByRole('radio', { name: 'System default' }))

    await waitFor(() => expect(document.documentElement).not.toHaveAttribute('data-theme'))
  })

  it('saves a folder picked in the native dialog, and can go back to the default', async () => {
    const backend = installFakeBackend()
    vi.mocked(open).mockResolvedValue('C:\\Meetings')
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: 'Choose folder…' }))

    await waitFor(() => expect(screen.getByTestId('storage-folder')).toHaveTextContent('C:\\Meetings'))
    expect(backend.settings.storageFolder).toBe('C:\\Meetings')
    expect(screen.getByText('This is a folder you chose.', { exact: false })).toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: 'Use default folder' }))

    await waitFor(() =>
      expect(screen.getByTestId('storage-folder')).toHaveTextContent(DEFAULT_INFO.defaultStorageFolder),
    )
    expect(backend.settings.storageFolder).toBeNull()
  })

  it('changes nothing when the folder dialog is cancelled', async () => {
    const backend = installFakeBackend()
    vi.mocked(open).mockResolvedValue(null)
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: 'Choose folder…' }))

    await waitFor(() => expect(open).toHaveBeenCalled())
    expect(backend.updates()).toEqual([])
    expect(screen.getByTestId('storage-folder')).toHaveTextContent(DEFAULT_INFO.defaultStorageFolder)
  })

  it('shows the backend’s message when a value is rejected, and keeps the old value on screen', async () => {
    installFakeBackend({
      updateError: { kind: 'invalidSetting', message: "invalid setting 'storageFolder': the folder path must be absolute" },
    })
    await openSettings()

    await userEvent.click(screen.getByRole('switch', { name: /consent/i }))

    expect(await screen.findByRole('alert')).toHaveTextContent('the folder path must be absolute')
    expect(screen.getByRole('switch', { name: /consent/i })).toBeChecked()
  })

  it('does not lose a change when two are made before the first save finishes', async () => {
    const backend = installFakeBackend()
    await openSettings()

    // Both clicks happen before any save has been answered.
    fireEvent.click(screen.getByRole('switch', { name: /consent/i }))
    fireEvent.click(screen.getByRole('radio', { name: 'Dark' }))

    await waitFor(() => expect(backend.updates()).toHaveLength(2))
    expect(backend.settings).toEqual({ ...DEFAULT_SETTINGS, theme: 'dark', consentReminder: false })
  })

  it('shows startup warnings from the backend', async () => {
    installFakeBackend({ info: { startupWarnings: ['File logging is not running.'] } })

    await openSettings()

    expect(screen.getByRole('region', { name: 'Startup warnings' })).toHaveTextContent('File logging is not running.')
  })

  it('talks to the backend only through the registered commands', async () => {
    const backend = installFakeBackend()
    await openSettings()
    await screen.findByRole('table')

    const commands = new Set(backend.calls.map((call) => call.command))
    // recording_status: every screen shares the recorder status, which is asked for once at startup.
    expect(commands).toEqual(new Set(['get_app_info', 'get_settings', 'get_capabilities', 'recording_status']))
    expect(invoke).toHaveBeenCalled()
  })
})

describe('Settings screen: shortcuts', () => {
  it('shows the two shortcuts', async () => {
    installFakeBackend()

    await openSettings()

    expect(screen.getByRole('button', { name: /Start or stop recording: Ctrl\+Shift\+S/ })).toHaveTextContent('Ctrl+Shift+S')
    expect(screen.getByRole('button', { name: /Pause or resume: Ctrl\+Shift\+P/ })).toHaveTextContent('Ctrl+Shift+P')
  })

  it('saves a new combination pressed after choosing the button, and keeps the other values', async () => {
    const backend = installFakeBackend()
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: /Start or stop recording/ }))
    expect(screen.getByRole('button', { name: /Start or stop recording/ })).toHaveTextContent('Press the new keys')
    await userEvent.keyboard('{Control>}{Alt>}r{/Alt}{/Control}')

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Start or stop recording/ })).toHaveTextContent('Ctrl+Alt+R'),
    )
    expect(backend.updates()).toEqual([
      { ...DEFAULT_SETTINGS, hotkeys: { startStop: 'Ctrl+Alt+R', pauseResume: 'Ctrl+Shift+P' } },
    ])
  })

  it('explains a combination that is not allowed and saves nothing', async () => {
    const backend = installFakeBackend()
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: /Pause or resume/ }))
    await userEvent.keyboard('{Shift>}p{/Shift}')

    expect(await screen.findByRole('alert')).toHaveTextContent('Hold Ctrl, Alt')
    expect(backend.updates()).toEqual([])
  })

  it('Escape cancels the change', async () => {
    const backend = installFakeBackend()
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: /Pause or resume/ }))
    await userEvent.keyboard('{Escape}')

    expect(screen.getByRole('button', { name: /Pause or resume/ })).toHaveTextContent('Ctrl+Shift+P')
    expect(backend.updates()).toEqual([])
  })

  it('shows the backend message when another program already owns the combination', async () => {
    installFakeBackend({
      updateError: {
        kind: 'invalidSetting',
        message:
          "invalid setting 'hotkeys': The start/stop shortcut \"Ctrl+Alt+R\" cannot be used: another program may already be using it.",
      },
    })
    await openSettings()

    await userEvent.click(screen.getByRole('button', { name: /Start or stop recording/ }))
    await userEvent.keyboard('{Control>}{Alt>}r{/Alt}{/Control}')

    expect(await screen.findByRole('alert')).toHaveTextContent('another program may already be using it')
    expect(screen.getByRole('button', { name: /Start or stop recording/ })).toHaveTextContent('Ctrl+Shift+S')
  })
})
