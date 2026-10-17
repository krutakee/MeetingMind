import type { DeviceLists, MonitorInfo, Quality, RecordingSetup } from '@/lib/ipc'

/** The same defaults as `RecordingSetup::default()` in src-tauri/src/recording/setup.rs. */
export const DEFAULT_SETUP: RecordingSetup = {
  title: '',
  mode: 'screenAudio',
  monitorId: null,
  microphone: true,
  microphoneId: null,
  systemAudio: true,
  quality: 'p1080',
  fps: 15,
}

export const FRAME_RATES = [5, 10, 15, 30] as const

export const QUALITY_LABELS: Record<Quality, string> = {
  p720: '720p (smaller files)',
  p1080: '1080p (recommended)',
  native: 'Full size of the monitor',
}

export function describeMonitor(monitor: MonitorInfo): string {
  const size = `${monitor.width}×${monitor.height}`
  return monitor.isPrimary ? `${monitor.name} (${size}, main monitor)` : `${monitor.name} (${size})`
}

/** The monitor the setup records: the one chosen, or the main one. */
export function chosenMonitor(setup: RecordingSetup, devices: DeviceLists): MonitorInfo | undefined {
  return setup.monitorId
    ? devices.monitors.find((monitor) => monitor.id === setup.monitorId)
    : (devices.monitors.find((monitor) => monitor.isPrimary) ?? devices.monitors[0])
}

/**
 * A setup remembered from last time, made safe for today's devices: a monitor or microphone that is no longer
 * connected falls back to the main monitor and the default microphone (the choice is not silently kept).
 */
export function reconcileWithDevices(setup: RecordingSetup, devices: DeviceLists): RecordingSetup {
  const monitorGone = setup.monitorId !== null && !devices.monitors.some((m) => m.id === setup.monitorId)
  const microphoneGone = setup.microphoneId !== null && !devices.microphones.some((m) => m.id === setup.microphoneId)
  return {
    ...setup,
    monitorId: monitorGone ? null : setup.monitorId,
    microphoneId: microphoneGone ? null : setup.microphoneId,
  }
}
