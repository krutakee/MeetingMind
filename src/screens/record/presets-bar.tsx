import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Select, TextInput } from '@/components/ui/form-controls'
import { Label } from '@/components/ui/label'
import { errorMessage, ipc, type Preset, type RecordingSetup } from '@/lib/ipc'

/**
 * Saved setups. Choosing one fills in the form (the title is kept: it belongs to this meeting). "Save as preset"
 * stores the current choices under a name; saving under an existing name replaces that preset.
 */
export function PresetsBar({
  presets,
  setup,
  onApply,
  onChanged,
}: {
  presets: Preset[]
  setup: RecordingSetup
  onApply: (setup: RecordingSetup) => void
  /** The list of presets changed (saved or deleted): the owner reloads it. */
  onChanged: () => void
}) {
  const [selected, setSelected] = useState('')
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)

  async function save() {
    setError(null)
    try {
      const saved = await ipc.savePreset(name, setup)
      setName('')
      setSelected(String(saved.id))
      onChanged()
    } catch (failure) {
      setError(errorMessage(failure))
    }
  }

  async function remove() {
    const preset = presets.find((p) => String(p.id) === selected)
    if (!preset) return
    setError(null)
    try {
      await ipc.deletePreset(preset.id)
      setSelected('')
      onChanged()
    } catch (failure) {
      setError(errorMessage(failure))
    }
  }

  return (
    <fieldset className="space-y-3">
      <legend className="text-sm font-medium">Presets</legend>
      <div className="flex flex-wrap items-end gap-3">
        <div className="min-w-48 flex-1 space-y-1">
          <Label htmlFor="preset-select">Use a saved preset</Label>
          <Select
            id="preset-select"
            value={selected}
            disabled={presets.length === 0}
            onChange={(event) => {
              setSelected(event.target.value)
              const preset = presets.find((p) => String(p.id) === event.target.value)
              if (preset) onApply({ ...preset.setup, title: setup.title })
            }}
          >
            <option value="">{presets.length === 0 ? 'No presets saved yet' : 'Choose a preset…'}</option>
            {presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
          </Select>
        </div>
        <Button variant="outline" disabled={!selected} onClick={() => void remove()}>
          Delete preset
        </Button>
      </div>
      <div className="flex flex-wrap items-end gap-3">
        <div className="min-w-48 flex-1 space-y-1">
          <Label htmlFor="preset-name">Save the current choices as</Label>
          <TextInput
            id="preset-name"
            value={name}
            maxLength={60}
            placeholder="Preset name"
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <Button variant="outline" disabled={name.trim() === ''} onClick={() => void save()}>
          Save preset
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
    </fieldset>
  )
}
