import { useState } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'
import { Modal } from '@/components/ui/modal'
import { errorMessage, ipc } from '@/lib/ipc'

export interface PlayableRecording {
  title: string
  file: string
  folder: string
  hasVideo: boolean
}

/**
 * Plays a recording inside the app. The file is read through Tauri's asset protocol, which only serves the storage
 * folder. If the player cannot play it, the folder can still be opened.
 */
export function PlayerDialog({ recording, onClose }: { recording: PlayableRecording | null; onClose: () => void }) {
  const [problem, setProblem] = useState<string | null>(null)

  if (!recording) return null
  const source = convertFileSrc(recording.file)
  const failed = () => setProblem('This file could not be played here. You can open its folder and use another player.')

  async function openFolder() {
    if (!recording) return
    try {
      await ipc.openFolder(recording.folder)
    } catch (failure) {
      setProblem(errorMessage(failure))
    }
  }

  return (
    <Modal
      open
      title={recording.title}
      onClose={() => {
        setProblem(null)
        onClose()
      }}
    >
      {recording.hasVideo ? (
        <video key={source} src={source} controls autoPlay onError={failed} className="max-h-[60vh] w-full rounded-md bg-black" />
      ) : (
        <audio key={source} src={source} controls autoPlay onError={failed} className="w-full" />
      )}
      {problem && (
        <p role="alert" className="text-sm text-destructive">
          {problem}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={() => void openFolder()}>
          Open folder
        </Button>
        <Button
          onClick={() => {
            setProblem(null)
            onClose()
          }}
          autoFocus
        >
          Close
        </Button>
      </div>
    </Modal>
  )
}
