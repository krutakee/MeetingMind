import { useEffect, useState, type ReactNode } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Switch } from '@/components/ui/switch'
import {
  errorMessage,
  ipc,
  type CapabilityId,
  type CapabilityReport,
  type CapabilityStatus,
  type Settings,
  type Theme,
} from '@/lib/ipc'
import { THEME_LABELS } from '@/lib/theme'
import { useAppState } from '@/state/app-state'

const CAPABILITY_LABELS: Record<CapabilityId, string> = {
  ram: 'Memory (RAM)',
  cpu_cores: 'Processor threads',
  system_audio_capture: 'System audio capture',
  capture_exclusion: 'Hide MeetingMind from recordings',
  gpu_backend: 'GPU acceleration',
  video_encoders: 'Video encoders',
}

function statusText(status: CapabilityStatus, value: string | null): string {
  switch (status) {
    case 'detected':
      return value ? `Detected: ${value}` : 'Detected'
    case 'unknown':
      return 'Unknown'
    case 'notYetImplemented':
      return 'Not yet implemented'
  }
}

export function SettingsScreen() {
  const { settings } = useAppState()
  // Settings are only missing when the app started with an error; the app shows an error screen then.
  if (!settings) return null
  return <SettingsForm settings={settings} />
}

function SettingsForm({ settings }: { settings: Settings }) {
  const { info, saveSettings } = useAppState()
  const [error, setError] = useState<string | null>(null)

  async function update(change: Partial<Settings>) {
    setError(null)
    try {
      await saveSettings(change)
    } catch (failure) {
      setError(errorMessage(failure))
    }
  }

  async function chooseFolder() {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        defaultPath: settings.storageFolder ?? undefined,
        title: 'Choose where meetings are saved',
      })
      if (typeof picked === 'string') await update({ storageFolder: picked })
    } catch (failure) {
      setError(errorMessage(failure))
    }
  }

  const usingDefaultFolder = settings.storageFolder === null

  return (
    <div className="max-w-3xl space-y-10">
      <h1 className="text-2xl font-semibold">Settings</h1>

      {info.startupWarnings.length > 0 && (
        <section aria-label="Startup warnings" className="rounded-md border-2 border-border p-4">
          <ul className="list-disc space-y-1 pl-5">
            {info.startupWarnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        </section>
      )}

      {error && (
        <p role="alert" className="rounded-md border-2 border-destructive p-4 text-destructive">
          {error}
        </p>
      )}

      <Section title="Appearance">
        <RadioGroup
          aria-label="Theme"
          value={settings.theme}
          onValueChange={(value) => void update({ theme: value as Theme })}
        >
          {(Object.keys(THEME_LABELS) as Theme[]).map((theme) => (
            <div key={theme} className="flex items-center gap-3">
              <RadioGroupItem value={theme} id={`theme-${theme}`} />
              <Label htmlFor={`theme-${theme}`}>{THEME_LABELS[theme]}</Label>
            </div>
          ))}
        </RadioGroup>
      </Section>

      <Section title="Storage">
        <p className="text-sm text-muted-foreground">Meetings are saved in this folder.</p>
        <p className="break-all rounded-md bg-muted p-3 font-mono text-sm" data-testid="storage-folder">
          {settings.storageFolder ?? info.defaultStorageFolder}
        </p>
        <p className="text-sm text-muted-foreground">
          {usingDefaultFolder ? 'This is the default folder.' : 'This is a folder you chose.'} MeetingMind does not
          create it until recording is available.
        </p>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => void chooseFolder()}>
            Choose folder…
          </Button>
          <Button
            variant="outline"
            disabled={usingDefaultFolder}
            onClick={() => void update({ storageFolder: null })}
          >
            Use default folder
          </Button>
        </div>
      </Section>

      <Section title="Privacy">
        <div className="flex items-start gap-3">
          <Switch
            id="consent-reminder"
            checked={settings.consentReminder}
            onCheckedChange={(checked) => void update({ consentReminder: checked })}
          />
          <div>
            <Label htmlFor="consent-reminder">Remind me to get participants’ consent before recording</Label>
            <p className="mt-1 text-sm text-muted-foreground">
              Shown before each recording starts. Recording is not available yet, so this has no effect for now.
            </p>
          </div>
        </div>
      </Section>

      <Section title="System capabilities">
        <Capabilities />
      </Section>

      <Section title="About">
        <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
          <dt className="text-muted-foreground">Version</dt>
          <dd>{info.version}</dd>
          <dt className="text-muted-foreground">Database schema</dt>
          <dd>{info.schemaVersion ?? 'unknown'}</dd>
          <dt className="text-muted-foreground">App data folder</dt>
          <dd className="break-all font-mono">{info.dataDir}</dd>
          <dt className="text-muted-foreground">Log folder</dt>
          <dd className="break-all font-mono">{info.logDir}</dd>
        </dl>
      </Section>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  const id = `section-${title.toLowerCase().replace(/\s+/g, '-')}`
  return (
    <section aria-labelledby={id} className="space-y-3">
      <h2 id={id} className="text-lg font-semibold">
        {title}
      </h2>
      {children}
    </section>
  )
}

function Capabilities() {
  const [report, setReport] = useState<CapabilityReport | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    ipc
      .getCapabilities()
      .then((result) => {
        if (!cancelled) setReport(result)
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(errorMessage(failure))
      })
    return () => {
      cancelled = true
    }
  }, [])

  if (error) {
    return (
      <p role="alert" className="text-destructive">
        {error}
      </p>
    )
  }
  if (!report) return <p className="text-muted-foreground">Checking this computer…</p>

  return (
    <table className="w-full border-collapse text-left text-sm">
      <caption className="sr-only">What MeetingMind found out about this computer ({report.platform})</caption>
      <thead>
        <tr className="border-b border-border">
          <th scope="col" className="py-2 pr-4 font-medium">
            Capability
          </th>
          <th scope="col" className="py-2 pr-4 font-medium">
            Status
          </th>
          <th scope="col" className="py-2 font-medium">
            Details
          </th>
        </tr>
      </thead>
      <tbody>
        {report.capabilities.map((capability) => (
          <tr key={capability.id} className="border-b border-border align-top">
            <th scope="row" className="py-2 pr-4 font-normal">
              {CAPABILITY_LABELS[capability.id]}
            </th>
            <td className="py-2 pr-4">{statusText(capability.status, capability.value)}</td>
            <td className="py-2 text-muted-foreground">{capability.detail}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
