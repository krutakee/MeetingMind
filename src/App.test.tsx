import { invoke } from '@tauri-apps/api/core'
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { installFakeBackend } from '@/test/fake-backend'
import { renderApp } from '@/test/render-app'

// TEST DOUBLES: the Tauri bridge is replaced for every test (src/test/setup.ts, src/test/fake-backend.ts); the native
// folder dialog is replaced here.
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

describe('App shell', () => {
  it('opens on the Record screen with the recording options', async () => {
    installFakeBackend()

    renderApp('/')

    expect(await screen.findByRole('heading', { name: 'Record' })).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: /Start recording/ })).toBeInTheDocument()
    expect(screen.queryByText('Not yet implemented')).not.toBeInTheDocument()
  })

  it('navigates between Record, Library and Settings and marks the current page', async () => {
    installFakeBackend()
    renderApp('/')
    await screen.findByRole('heading', { name: 'Record' })
    const nav = screen.getByRole('navigation', { name: 'Main' })

    await userEvent.click(within(nav).getByRole('link', { name: 'Library' }))

    expect(await screen.findByRole('heading', { name: 'Library' })).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Status' })).toHaveTextContent('Not yet implemented')
    expect(within(nav).getByRole('link', { name: 'Library' })).toHaveAttribute('aria-current', 'page')
    expect(within(nav).getByRole('link', { name: 'Record' })).not.toHaveAttribute('aria-current')

    await userEvent.click(within(nav).getByRole('link', { name: 'Settings' }))

    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('sends unknown addresses to the Record screen', async () => {
    installFakeBackend()

    renderApp('/no/such/page')

    expect(await screen.findByRole('heading', { name: 'Record' })).toBeInTheDocument()
  })

  it('shows the product name from the backend (it is configured in one place)', async () => {
    installFakeBackend({ info: { productName: 'AcmeMind' } })

    renderApp('/')

    expect(await screen.findByText('AcmeMind')).toBeInTheDocument()
  })

  it('has a skip link and one main landmark', async () => {
    installFakeBackend()

    renderApp('/')

    await screen.findByRole('heading', { name: 'Record' })
    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main')
    expect(screen.getAllByRole('main')).toHaveLength(1)
  })

  it('shows the startup error instead of the screens when the database could not open', async () => {
    installFakeBackend({
      info: { startupError: 'the database was created by a newer version of MeetingMind', schemaVersion: null },
    })

    renderApp('/')

    expect(await screen.findByRole('heading', { name: 'MeetingMind could not start' })).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('created by a newer version')
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument()
    // The settings are never requested when there is no database.
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).not.toContain('get_settings')
  })

  it('says so when the backend cannot be reached', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('backend is not running'))

    renderApp('/')

    expect(await screen.findByRole('heading', { name: 'MeetingMind could not reach its backend' })).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('backend is not running')
  })

  it('shows a status message while starting', () => {
    vi.mocked(invoke).mockReturnValue(new Promise(() => {}))

    renderApp('/')

    expect(screen.getByRole('status')).toHaveTextContent('Starting…')
  })
})
