import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { getCurrentWindow } from '@tauri-apps/api/window'
import App from '@/App'
import { ControlWindow } from '@/control/control-window'
import '@/index.css'

const root = document.getElementById('root')
if (!root) throw new Error('index.html is missing the #root element')

/** The backend opens a second window, labelled "control", on this same page while recording. */
function windowLabel(): string {
  try {
    return getCurrentWindow().label
  } catch {
    return 'main' // Opened in a plain browser (no Tauri): behave like the main window.
  }
}

createRoot(root).render(<StrictMode>{windowLabel() === 'control' ? <ControlWindow /> : <App />}</StrictMode>)
