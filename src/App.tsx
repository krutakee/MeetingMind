import { HashRouter, Navigate, Route, Routes } from 'react-router-dom'
import { Layout } from '@/components/layout'
import { LibraryScreen } from '@/screens/library-screen'
import { RecordScreen } from '@/screens/record-screen'
import { SettingsScreen } from '@/screens/settings-screen'
import { StartupFailedScreen } from '@/screens/startup-screens'
import { AppProvider, useAppState } from '@/state/app-state'
import { RecorderStatusProvider } from '@/state/recorder-status'

/** The screens. Needs a router and <AppProvider> around it (App.tsx wires them; tests use their own router). */
export function AppRoutes() {
  const { info } = useAppState()
  if (info.startupError) {
    return (
      <StartupFailedScreen
        title="MeetingMind could not start"
        message={info.startupError}
        logDir={info.logDir}
      />
    )
  }
  return (
    <RecorderStatusProvider>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Navigate to="/record" replace />} />
          <Route path="record" element={<RecordScreen />} />
          <Route path="library" element={<LibraryScreen />} />
          <Route path="settings" element={<SettingsScreen />} />
          <Route path="*" element={<Navigate to="/record" replace />} />
        </Route>
      </Routes>
    </RecorderStatusProvider>
  )
}

export default function App() {
  return (
    <HashRouter>
      <AppProvider>
        <AppRoutes />
      </AppProvider>
    </HashRouter>
  )
}
