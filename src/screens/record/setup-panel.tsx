import { useCallback, useEffect, useState } from 'react'
import { Circle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Select, TextInput } from '@/components/ui/form-controls'
import { Label } from '@/components/ui/label'
import { ConfirmDialog } from '@/components/ui/modal'
import { Notice } from '@/components/ui/notice'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Switch } from '@/components/ui/switch'
import { formatBytes } from '@/lib/format'
import {
  errorKind,
  errorMessage,
  ipc,
  type DeviceLists,
  type Estimate,
  type Preset,
  type Quality,
  type RecordingMode,
  type RecordingSetup,
} from '@/lib/ipc'
import {
  DEFAULT_SETUP,
  FRAME_RATES,
  QUALITY_LABELS,
  chosenMonitor,
  cloudSyncWarning,
  describeMonitor,
  reconcileWithDevices,
} from '@/lib/setup'
import { MonitorPreview } from '@/screens/record/monitor-preview'
import { PresetsBar } from '@/screens/record/presets-bar'
import { useAppState } from '@/state/app-state'
import { useRecorderStatus } from '@/state/recorder-status'

/** The choices before a recording, the live preview, what it will cost, and the Start button. */
export function SetupPanel() {
  const { settings } = useAppState()
  const { refresh } = useRecorderStatus()
  const [setup, setSetup] = useState<RecordingSetup | null>(null)
  const [devices, setDevices] = useState<DeviceLists | null>(null)
  const [presets, setPresets] = useState<Preset[]>([])
  const [estimate, setEstimate] = useState<Estimate | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [starting, setStarting] = useState(false)
  const [askingConsent, setAskingConsent] = useState(false)

  const reloadPresets = useCallback(() => {
    ipc
      .listPresets()
      .then(setPresets)
      .catch((failure: unknown) => setError(errorMessage(failure)))
  }, [])

  useEffect(() => {
    let cancelled = false
    Promise.all([ipc.getLastSetup(), ipc.listDevices(), ipc.listPresets()])
      .then(([last, found, saved]) => {
        if (cancelled) return
        setDevices(found)
        setPresets(saved)
        setSetup(reconcileWithDevices(last ?? DEFAULT_SETUP, found))
      })
      .catch((failure: unknown) => {
        if (!cancelled) setLoadError(errorMessage(failure))
      })
    return () => {
      cancelled = true
    }
  }, [])

  // What the choices cost. The title does not matter for that, so typing it does not ask again.
  const estimateKey = setup ? JSON.stringify({ ...setup, title: '' }) : null
  useEffect(() => {
    if (!estimateKey) return
    let cancelled = false
    const timer = setTimeout(() => {
      ipc
        .estimateRecording(JSON.parse(estimateKey) as RecordingSetup)
        .then((result) => {
          if (!cancelled) setEstimate(result)
        })
        .catch(() => {
          if (!cancelled) setEstimate(null)
        })
    }, 250)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [estimateKey])

  async function start(consentGiven: boolean) {
    if (!setup) return
    setError(null)
    setStarting(true)
    try {
      await ipc.startRecording(setup, consentGiven)
      await refresh()
    } catch (failure) {
      // The reminder was turned on in Settings after this screen last looked.
      if (errorKind(failure) === 'consentRequired') setAskingConsent(true)
      else setError(errorMessage(failure))
    } finally {
      setStarting(false)
    }
  }

  if (loadError) return <Notice tone="error">The recording options could not be loaded: {loadError}</Notice>
  if (!setup || !devices) return <p role="status" className="text-muted-foreground">Looking for screens and microphones…</p>

  const change = (changes: Partial<RecordingSetup>) => setSetup({ ...setup, ...changes })
  const screenMode = setup.mode === 'screenAudio'
  const nothingToRecord = !screenMode && !setup.microphone && !setup.systemAudio
  const monitor = chosenMonitor(setup, devices)
  const tooLittleSpace = estimate && estimate.freeBytes !== null && estimate.freeBytes < estimate.maxBytesPerHour

  return (
    <div className="space-y-8">
      {error && <Notice tone="error">{error}</Notice>}

      <div className="grid gap-8 lg:grid-cols-2">
        <div className="space-y-5">
          <div className="space-y-1">
            <Label htmlFor="title">Meeting title</Label>
            <TextInput
              id="title"
              value={setup.title}
              placeholder="Meeting"
              maxLength={120}
              onChange={(event) => change({ title: event.target.value })}
            />
          </div>

          <div className="space-y-2">
            <p id="mode-label" className="text-sm font-medium">
              What to record
            </p>
            <RadioGroup
              aria-labelledby="mode-label"
              value={setup.mode}
              onValueChange={(value) => change({ mode: value as RecordingMode })}
            >
              <div className="flex items-center gap-3">
                <RadioGroupItem value="screenAudio" id="mode-screen" />
                <Label htmlFor="mode-screen">Screen and audio</Label>
              </div>
              <div className="flex items-center gap-3">
                <RadioGroupItem value="audioOnly" id="mode-audio" />
                <Label htmlFor="mode-audio">Audio only</Label>
              </div>
            </RadioGroup>
          </div>

          {screenMode && (
            <div className="space-y-1">
              <Label htmlFor="monitor">Monitor</Label>
              <Select
                id="monitor"
                value={setup.monitorId ?? monitor?.id ?? ''}
                disabled={devices.monitors.length === 0}
                onChange={(event) => change({ monitorId: event.target.value })}
              >
                {devices.monitors.length === 0 && <option value="">No monitor found</option>}
                {devices.monitors.map((m) => (
                  <option key={m.id} value={m.id}>
                    {describeMonitor(m)}
                  </option>
                ))}
              </Select>
              {devices.monitorsProblem && <p className="text-sm text-destructive">{devices.monitorsProblem}</p>}
            </div>
          )}

          <div className="space-y-2">
            <div className="flex items-center gap-3">
              <Switch
                id="microphone"
                checked={setup.microphone}
                onCheckedChange={(checked) => change({ microphone: checked })}
              />
              <Label htmlFor="microphone">Record my microphone</Label>
            </div>
            {setup.microphone && (
              <div className="space-y-1 pl-12">
                <Label htmlFor="microphone-device">Microphone</Label>
                <Select
                  id="microphone-device"
                  value={setup.microphoneId ?? ''}
                  onChange={(event) => change({ microphoneId: event.target.value || null })}
                >
                  <option value="">Default microphone</option>
                  {devices.microphones.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                      {m.isDefault ? ' (default)' : ''}
                    </option>
                  ))}
                </Select>
                {devices.microphonesProblem && <p className="text-sm text-destructive">{devices.microphonesProblem}</p>}
              </div>
            )}
          </div>

          <div className="flex items-start gap-3">
            <Switch
              id="system-audio"
              checked={setup.systemAudio}
              onCheckedChange={(checked) => change({ systemAudio: checked })}
            />
            <div>
              <Label htmlFor="system-audio">Record system audio</Label>
              <p className="mt-1 text-sm text-muted-foreground">
                Everything the computer plays, such as the other people in a call.
              </p>
            </div>
          </div>

          {screenMode && (
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-1">
                <Label htmlFor="quality">Video quality</Label>
                <Select
                  id="quality"
                  value={setup.quality}
                  onChange={(event) => change({ quality: event.target.value as Quality })}
                >
                  {(Object.keys(QUALITY_LABELS) as Quality[]).map((quality) => (
                    <option key={quality} value={quality}>
                      {QUALITY_LABELS[quality]}
                    </option>
                  ))}
                </Select>
              </div>
              <div className="space-y-1">
                <Label htmlFor="fps">Frames per second</Label>
                <Select id="fps" value={setup.fps} onChange={(event) => change({ fps: Number(event.target.value) })}>
                  {FRAME_RATES.map((rate) => (
                    <option key={rate} value={rate}>
                      {rate}
                    </option>
                  ))}
                </Select>
              </div>
            </div>
          )}

          <PresetsBar presets={presets} setup={setup} onApply={setSetup} onChanged={reloadPresets} />
        </div>

        <div className="space-y-4">
          {screenMode && devices.monitors.length > 0 && <MonitorPreview monitorId={monitor?.id ?? null} />}

          <section aria-label="Estimate" className="space-y-1 rounded-md border border-border bg-card p-4 text-sm">
            {estimate ? (
              <>
                <p>
                  Up to <strong>{formatBytes(estimate.maxBytesPerHour)}</strong> per hour
                  {estimate.videoWidth && estimate.videoHeight
                    ? `, video ${estimate.videoWidth}×${estimate.videoHeight}`
                    : ', audio only'}
                  . This is an upper bound; the real size is usually smaller.
                </p>
                <p className="text-muted-foreground">
                  Saved in {estimate.storageFolder}
                  {estimate.freeBytes !== null && `, ${formatBytes(estimate.freeBytes)} free`}.
                </p>
              </>
            ) : (
              <p className="text-muted-foreground">Working out how much space this needs…</p>
            )}
          </section>
          {estimate?.storageSyncedBy && <Notice tone="warning">{cloudSyncWarning(estimate.storageSyncedBy, 'record')}</Notice>}
          {tooLittleSpace && (
            <Notice tone="warning">
              There is less free space than one hour of this recording can need. Free some space or choose a lower
              quality.
            </Notice>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-4">
        <Button disabled={starting || nothingToRecord} onClick={() => (settings?.consentReminder ? setAskingConsent(true) : void start(false))}>
          <Circle aria-hidden="true" className="size-4 fill-current" />
          {starting ? 'Starting…' : 'Start recording'}
        </Button>
        <p className="text-sm text-muted-foreground">
          {nothingToRecord
            ? 'Choose the microphone, system audio, or both.'
            : `Shortcut: ${settings?.hotkeys.startStop ?? 'Ctrl+Shift+S'} (works in any program).`}
        </p>
      </div>

      <ConfirmDialog
        open={askingConsent}
        title="Before you record"
        confirmLabel="Start recording"
        busy={starting}
        onCancel={() => setAskingConsent(false)}
        onConfirm={() => {
          setAskingConsent(false)
          void start(true)
        }}
      >
        <p>Make sure everyone in the meeting knows about it and agrees to being recorded.</p>
        <p className="text-muted-foreground">You can turn this reminder off in Settings.</p>
      </ConfirmDialog>
    </div>
  )
}
