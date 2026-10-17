import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { act } from 'react'
import { describe, expect, it, vi } from 'vitest'
import type { RecordingSetup } from '@/lib/ipc'
import {
  DEFAULT_ESTIMATE,
  DEFAULT_SETTINGS,
  FINISHED,
  IDLE_STATUS,
  RECORDING_STATUS,
  installFakeBackend,
} from '@/test/fake-backend'
import { renderApp } from '@/test/render-app'

// TEST DOUBLES: the Tauri bridge is replaced for every test (src/test/setup.ts, src/test/fake-backend.ts); the native
// folder dialog is replaced here.
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

async function openRecord() {
  renderApp('/record')
  await screen.findByRole('heading', { name: 'Record' })
}

const startButton = () => screen.findByRole('button', { name: /Start recording/ })

describe('Record screen: setting up', () => {
  it('lists the monitors and microphones and starts with the recommended choices', async () => {
    installFakeBackend()

    await openRecord()

    expect(await screen.findByRole('combobox', { name: 'Monitor' })).toHaveDisplayValue('HP 27xw (1920×1080, main monitor)')
    expect(screen.getByRole('combobox', { name: 'Microphone' })).toHaveDisplayValue('Default microphone')
    expect(screen.getByRole('combobox', { name: 'Video quality' })).toHaveDisplayValue('1080p (recommended)')
    expect(screen.getByRole('combobox', { name: 'Frames per second' })).toHaveDisplayValue('15')
    expect(screen.getByRole('switch', { name: 'Record my microphone' })).toBeChecked()
    expect(screen.getByRole('switch', { name: 'Record system audio' })).toBeChecked()
    expect(screen.getByRole('radio', { name: 'Screen and audio' })).toBeChecked()
  })

  it('shows what the choices cost as an upper bound, with the free space', async () => {
    installFakeBackend()

    await openRecord()

    const estimate = await screen.findByRole('region', { name: 'Estimate' })
    await waitFor(() => expect(estimate).toHaveTextContent('Up to 3.3 GB per hour'))
    expect(estimate).toHaveTextContent('video 1920×1080')
    expect(estimate).toHaveTextContent('upper bound')
    expect(estimate).toHaveTextContent('200.0 GB free')
  })

  it('warns when there is less room than an hour of recording can need', async () => {
    installFakeBackend({
      estimate: {
        maxBytesPerHour: 3.3 * 1024 ** 3,
        videoWidth: 1920,
        videoHeight: 1080,
        freeBytes: 1024 ** 3,
        storageFolder: 'C:\\Meetings',
        storageSyncedBy: null,
      },
    })

    await openRecord()

    expect(await screen.findByText(/less free space than one hour/)).toBeInTheDocument()
  })

  it('warns that a cloud storage program uploads the storage folder', async () => {
    installFakeBackend({
      estimate: { ...DEFAULT_ESTIMATE, storageFolder: 'C:\\Users\\test\\OneDrive\\Meetings', storageSyncedBy: 'OneDrive' },
    })

    await openRecord()

    const warning = await screen.findByText(/synced by OneDrive/)
    expect(warning).toHaveTextContent('uploaded to its servers')
    expect(warning).toHaveTextContent('never sends anything anywhere')
    expect(warning.closest('[role="alert"]')).not.toBeNull()
  })

  it('shows no cloud warning for an ordinary folder', async () => {
    installFakeBackend()

    await openRecord()
    await screen.findByRole('region', { name: 'Estimate' })

    expect(screen.queryByText(/synced by/)).not.toBeInTheDocument()
  })

  it('hides the screen options for an audio-only recording and asks for a source', async () => {
    installFakeBackend()
    await openRecord()
    await screen.findByRole('combobox', { name: 'Monitor' })

    await userEvent.click(screen.getByRole('radio', { name: 'Audio only' }))

    expect(screen.queryByRole('combobox', { name: 'Monitor' })).not.toBeInTheDocument()
    expect(screen.queryByRole('combobox', { name: 'Video quality' })).not.toBeInTheDocument()

    await userEvent.click(screen.getByRole('switch', { name: 'Record my microphone' }))
    await userEvent.click(screen.getByRole('switch', { name: 'Record system audio' }))

    expect(await startButton()).toBeDisabled()
    expect(screen.getByText('Choose the microphone, system audio, or both.')).toBeInTheDocument()
  })

  it('starts from what was used last time, but not a monitor that is no longer connected', async () => {
    installFakeBackend({
      lastSetup: {
        title: '',
        mode: 'screenAudio',
        monitorId: '\\\\.\\DISPLAY9',
        microphone: false,
        microphoneId: null,
        systemAudio: true,
        quality: 'p720',
        fps: 10,
      },
    })

    await openRecord()

    expect(await screen.findByRole('combobox', { name: 'Monitor' })).toHaveDisplayValue('HP 27xw (1920×1080, main monitor)')
    expect(screen.getByRole('combobox', { name: 'Video quality' })).toHaveDisplayValue('720p (smaller files)')
    expect(screen.getByRole('combobox', { name: 'Frames per second' })).toHaveDisplayValue('10')
    expect(screen.getByRole('switch', { name: 'Record my microphone' })).not.toBeChecked()
  })

  it('says why the screens or microphones could not be listed', async () => {
    installFakeBackend({
      devices: {
        monitors: [],
        monitorsProblem: 'the screen could not be listed',
        microphones: [],
        microphonesProblem: 'the audio service is not running',
      },
    })

    await openRecord()

    expect(await screen.findByText('the screen could not be listed')).toBeInTheDocument()
    expect(screen.getByText('the audio service is not running')).toBeInTheDocument()
  })
})

describe('Record screen: starting', () => {
  it('asks for consent first, and starts only after it is given', async () => {
    const backend = installFakeBackend()
    await openRecord()
    await userEvent.type(await screen.findByLabelText('Meeting title'), 'Weekly sync')

    await userEvent.click(await startButton())

    const dialog = await screen.findByRole('dialog', { name: 'Before you record' })
    expect(within(dialog).getByText(/agrees to being recorded/)).toBeInTheDocument()
    expect(backend.argsOf('start_recording')).toEqual([])

    await userEvent.click(within(dialog).getByRole('button', { name: 'Start recording' }))

    await waitFor(() => expect(backend.argsOf('start_recording')).toHaveLength(1))
    const { setup, consentGiven } = backend.argsOf('start_recording')[0] as { setup: RecordingSetup; consentGiven: boolean }
    expect(consentGiven).toBe(true)
    expect(setup).toMatchObject({ title: 'Weekly sync', mode: 'screenAudio', quality: 'p1080', fps: 15 })
    expect(await screen.findByRole('region', { name: 'Recording in progress' })).toBeInTheDocument()
  })

  it('does not start when consent is declined', async () => {
    const backend = installFakeBackend()
    await openRecord()
    await userEvent.click(await startButton())
    const dialog = await screen.findByRole('dialog', { name: 'Before you record' })

    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(backend.argsOf('start_recording')).toEqual([])
  })

  it('starts at once when the reminder is turned off', async () => {
    const backend = installFakeBackend({ settings: { ...DEFAULT_SETTINGS, consentReminder: false } })
    await openRecord()

    await userEvent.click(await startButton())

    await waitFor(() => expect(backend.argsOf('start_recording')).toHaveLength(1))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect((backend.argsOf('start_recording')[0] as { consentGiven: boolean }).consentGiven).toBe(false)
  })

  it('shows why a recording could not start and stays on the setup screen', async () => {
    installFakeBackend({
      settings: { ...DEFAULT_SETTINGS, consentReminder: false },
      startError: { kind: 'device', message: 'The microphone cannot be used: the device was removed' },
    })
    await openRecord()

    await userEvent.click(await startButton())

    expect(await screen.findByRole('alert')).toHaveTextContent('the device was removed')
    expect(screen.getByRole('button', { name: /Start recording/ })).toBeEnabled()
  })

  it('asks for consent when the backend says it is needed after all', async () => {
    installFakeBackend({
      settings: { ...DEFAULT_SETTINGS, consentReminder: false },
      startError: { kind: 'consentRequired', message: 'Confirm that everyone agrees' },
    })
    await openRecord()

    await userEvent.click(await startButton())

    expect(await screen.findByRole('dialog', { name: 'Before you record' })).toBeInTheDocument()
  })
})

describe('Record screen: presets', () => {
  it('saves the current choices under a name and applies a preset without touching the title', async () => {
    const backend = installFakeBackend()
    await openRecord()
    await userEvent.type(await screen.findByLabelText('Meeting title'), 'Today')
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Video quality' }), 'p720')
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Frames per second' }), '5')

    await userEvent.type(screen.getByLabelText('Save the current choices as'), 'Small and light')
    await userEvent.click(screen.getByRole('button', { name: 'Save preset' }))

    await waitFor(() => expect(backend.presets).toHaveLength(1))
    expect(backend.presets[0].setup).toMatchObject({ quality: 'p720', fps: 5, title: '' })

    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Video quality' }), 'native')
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Use a saved preset' }), 'Small and light')

    expect(screen.getByRole('combobox', { name: 'Video quality' })).toHaveDisplayValue('720p (smaller files)')
    expect(screen.getByLabelText('Meeting title')).toHaveValue('Today')
  })

  it('deletes the chosen preset', async () => {
    const backend = installFakeBackend({
      presets: [
        {
          id: 1,
          name: 'Standup',
          setup: { title: '', mode: 'audioOnly', monitorId: null, microphone: true, microphoneId: null, systemAudio: false, quality: 'p1080', fps: 15 },
        },
      ],
    })
    await openRecord()
    await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Use a saved preset' }), 'Standup')
    expect(screen.getByRole('radio', { name: 'Audio only' })).toBeChecked()

    await userEvent.click(screen.getByRole('button', { name: 'Delete preset' }))

    await waitFor(() => expect(backend.presets).toEqual([]))
    expect(await screen.findByRole('option', { name: 'No presets saved yet' })).toBeInTheDocument()
  })
})

describe('Record screen: recording', () => {
  it('shows the time, the levels and the controls, and pauses and resumes', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    await openRecord()

    const section = await screen.findByRole('region', { name: 'Recording in progress' })
    expect(within(section).getByRole('timer')).toHaveTextContent('1:05')
    expect(within(section).getByText('Recording')).toBeInTheDocument()
    expect(within(section).getByRole('meter', { name: 'Microphone level' })).toHaveAttribute('aria-valuenow', '40')
    expect(within(section).getByRole('meter', { name: 'System level' })).toHaveAttribute('aria-valuenow', '20')

    await userEvent.click(within(section).getByRole('button', { name: 'Pause' }))

    expect(await within(section).findByText('Paused')).toBeInTheDocument()
    expect(backend.argsOf('pause_recording')).toHaveLength(1)

    await userEvent.click(within(section).getByRole('button', { name: 'Resume' }))

    expect(await within(section).findByText('Recording')).toBeInTheDocument()
    expect(backend.argsOf('resume_recording')).toHaveLength(1)
  })

  it('follows the status the backend pushes', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    await openRecord()
    const section = await screen.findByRole('region', { name: 'Recording in progress' })

    act(() => backend.emitStatus({ ...RECORDING_STATUS, elapsedMs: 3_725_000, microphoneLevel: 0.9 }))

    expect(within(section).getByRole('timer')).toHaveTextContent('1:02:05')
    expect(within(section).getByRole('meter', { name: 'Microphone level' })).toHaveAttribute('aria-valuenow', '90')
  })

  it('shows every warning from the recording, with the serious ones as errors', async () => {
    installFakeBackend({
      status: {
        ...RECORDING_STATUS,
        warnings: [
          { id: 1, kind: 'sourceLost', stream: 'microphone', message: 'The microphone was unplugged. Recording continues without it.', atMediaMs: 5000 },
          { id: 2, kind: 'diskCritical', stream: null, message: 'The disk is almost full.', atMediaMs: 9000 },
          { id: 3, kind: 'ownWindowVisible', stream: null, message: 'MeetingMind’s windows may appear in the video.', atMediaMs: 0 },
        ],
      },
    })
    await openRecord()

    expect(await screen.findByText(/microphone was unplugged/)).toBeInTheDocument()
    expect(screen.getByText('The disk is almost full.')).toBeInTheDocument()
    expect(screen.getByText(/windows may appear in the video/)).toBeInTheDocument()
    expect(screen.getAllByRole('alert')).toHaveLength(3)
  })

  it('says the recording is being stopped when the disk is full', async () => {
    installFakeBackend({ status: { ...RECORDING_STATUS, autoStop: 'diskFull' } })
    await openRecord()

    expect(await screen.findByText(/being stopped and saved/)).toBeInTheDocument()
  })

  it('stops and saves, then offers to play it and to open its folder', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    await openRecord()

    await userEvent.click(await screen.findByRole('button', { name: /Stop and save/ }))

    expect(await screen.findByText('Recording saved')).toBeInTheDocument()
    expect(backend.argsOf('stop_recording')).toHaveLength(1)
    const banner = screen.getByText('Recording saved').closest('[role="status"]') as HTMLElement
    expect(banner).toHaveTextContent('Weekly sync')
    expect(banner).toHaveTextContent('30:00')

    await userEvent.click(within(banner).getByRole('button', { name: 'Open folder' }))
    expect(backend.argsOf('open_folder')).toEqual([{ path: FINISHED.folder }])

    await userEvent.click(within(banner).getByRole('button', { name: 'Play' }))
    const player = await screen.findByRole('dialog', { name: 'Weekly sync' })
    const video = player.querySelector('video')
    expect(video).not.toBeNull()
    expect(video?.getAttribute('src')).toBe(`http://asset.localhost/${encodeURIComponent(FINISHED.file)}`)
  })

  it('plays an audio-only recording in an audio player', async () => {
    installFakeBackend({
      recent: [
        {
          id: 1,
          title: 'Phone call',
          startedAt: '2026-10-16T09:05:00Z',
          durationMs: 600_000,
          folder: 'C:\\Meetings\\call',
          file: 'C:\\Meetings\\call\\recording.m4a',
          hasVideo: false,
        },
      ],
    })
    await openRecord()

    await userEvent.click(await screen.findByRole('button', { name: 'Play Phone call' }))

    const player = await screen.findByRole('dialog', { name: 'Phone call' })
    expect(player.querySelector('audio')).not.toBeNull()
    expect(player.querySelector('video')).toBeNull()
  })

  it('asks before throwing a recording away', async () => {
    const backend = installFakeBackend({ status: RECORDING_STATUS })
    await openRecord()
    await userEvent.click(await screen.findByRole('button', { name: /Discard/ }))

    const dialog = await screen.findByRole('dialog', { name: 'Discard this recording?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Keep recording' }))

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(backend.argsOf('cancel_recording')).toEqual([])

    await userEvent.click(screen.getByRole('button', { name: /Discard/ }))
    await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Discard recording' }))

    await waitFor(() => expect(backend.argsOf('cancel_recording')).toHaveLength(1))
    expect(await startButton()).toBeInTheDocument()
  })

  it('says so while the recording is being saved', async () => {
    installFakeBackend({ status: { ...RECORDING_STATUS, phase: 'finalizing' } })

    await openRecord()

    expect(await screen.findByText(/Saving the recording/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Start recording/ })).not.toBeInTheDocument()
  })

  it('tells the user when a recording could not be finished into one file', async () => {
    installFakeBackend({ status: { ...IDLE_STATUS, error: 'the tracks are kept in C:\\Meetings\\x' } })

    await openRecord()

    expect(await screen.findByText(/could not be finished into one file/)).toHaveTextContent('the tracks are kept')
  })
})

describe('Record screen: recent recordings', () => {
  it('lists them with play and open-folder, and says when a file is gone', async () => {
    const backend = installFakeBackend({
      recent: [
        { id: 1, title: 'Planning', startedAt: '2026-10-16T09:05:00Z', durationMs: 125_000, folder: 'C:\\M\\a', file: 'C:\\M\\a\\recording.mp4', hasVideo: true },
        { id: 2, title: 'Old one', startedAt: '2026-10-01T09:05:00Z', durationMs: null, folder: 'C:\\M\\b', file: null, hasVideo: false },
      ],
    })

    await openRecord()

    const list = await within(await screen.findByRole('region', { name: 'Recent recordings' })).findByRole('list')
    expect(within(list).getByText('Planning')).toBeInTheDocument()
    expect(within(list).getByText(/2:05/)).toBeInTheDocument()
    expect(within(list).getByText(/no longer there/)).toBeInTheDocument()
    expect(within(list).getByRole('button', { name: 'Play Old one' })).toBeDisabled()
    expect(backend.argsOf('list_recent_recordings')).toEqual([{ limit: 10 }])

    await userEvent.click(within(list).getByRole('button', { name: 'Open the folder of Planning' }))
    expect(backend.argsOf('open_folder')).toEqual([{ path: 'C:\\M\\a' }])
  })

  it('says plainly when there is nothing yet', async () => {
    installFakeBackend()

    await openRecord()

    expect(await screen.findByText(/Nothing recorded yet/)).toBeInTheDocument()
  })
})

describe('Record screen: recovery', () => {
  const unfinished = [{ id: 5, title: 'Board meeting', startedAt: '2026-10-15T14:00:00Z', folder: 'C:\\M\\board' }]

  it('offers to recover an unfinished recording found at launch', async () => {
    const backend = installFakeBackend({ unfinished })
    await openRecord()

    const dialog = await screen.findByRole('dialog', { name: 'Unfinished recording found' })
    expect(within(dialog).getByText('Board meeting')).toBeInTheDocument()

    await userEvent.click(within(dialog).getByRole('button', { name: 'Recover' }))

    await waitFor(() => expect(backend.argsOf('recover_recording')).toEqual([{ id: 5 }]))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.getByText(/Recovered "Board meeting" \(2:00\)/)).toBeInTheDocument()
  })

  it('asks again before deleting an unfinished recording', async () => {
    const backend = installFakeBackend({ unfinished })
    await openRecord()
    const dialog = await screen.findByRole('dialog', { name: 'Unfinished recording found' })

    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete Board meeting' }))
    expect(backend.argsOf('discard_recording')).toEqual([])
    await userEvent.click(within(dialog).getByRole('button', { name: 'Keep it' }))
    expect(backend.argsOf('discard_recording')).toEqual([])

    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete Board meeting' }))
    await userEvent.click(within(dialog).getByRole('button', { name: 'Yes, delete it' }))

    await waitFor(() => expect(backend.argsOf('discard_recording')).toEqual([{ id: 5 }]))
  })

  it('leaves it for later when the user decides to', async () => {
    const backend = installFakeBackend({ unfinished })
    await openRecord()
    const dialog = await screen.findByRole('dialog', { name: 'Unfinished recording found' })

    await userEvent.click(within(dialog).getByRole('button', { name: 'Decide later' }))

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(backend.argsOf('recover_recording')).toEqual([])
    expect(backend.argsOf('discard_recording')).toEqual([])
  })

  it('says why a recording could not be recovered and keeps the dialog open', async () => {
    installFakeBackend({
      unfinished,
      recoverError: { kind: 'unavailable', message: 'no usable recording was found in C:\\M\\board' },
    })
    await openRecord()
    const dialog = await screen.findByRole('dialog', { name: 'Unfinished recording found' })

    await userEvent.click(within(dialog).getByRole('button', { name: 'Recover' }))

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('no usable recording was found')
    expect(screen.getByRole('dialog', { name: 'Unfinished recording found' })).toBeInTheDocument()
  })

  it('shows no dialog when nothing is unfinished', async () => {
    installFakeBackend()

    await openRecord()
    await startButton()

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })
})
