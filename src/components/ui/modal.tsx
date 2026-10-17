import { useEffect, useId, useRef, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'

/**
 * A modal dialog built on the browser's own <dialog> element, which gives a focus trap, the Escape key and a layer
 * above the page without any library. Nothing is rendered while `open` is false.
 */
export function Modal({
  open,
  title,
  onClose,
  children,
}: {
  open: boolean
  title: string
  /** Called for Escape and for the backdrop; the owner decides whether that closes the dialog. */
  onClose: () => void
  children: ReactNode
}) {
  if (!open) return null
  return (
    <ModalBody title={title} onClose={onClose}>
      {children}
    </ModalBody>
  )
}

function ModalBody({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null)
  const titleId = useId()

  useEffect(() => {
    const dialog = ref.current
    if (dialog && !dialog.open) dialog.showModal()
  }, [])

  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      onCancel={(event) => {
        // The owner decides what Escape does; the browser must not close the dialog behind its back.
        event.preventDefault()
        onClose()
      }}
      onClick={(event) => {
        // A click on the backdrop reaches the dialog element itself, not its content.
        if (event.target === ref.current) onClose()
      }}
      className="m-auto w-[min(32rem,calc(100vw-2rem))] rounded-lg border-2 border-border bg-background p-0 text-foreground backdrop:bg-black/50"
    >
      <div className="space-y-4 p-6">
        <h2 id={titleId} className="text-lg font-semibold">
          {title}
        </h2>
        {children}
      </div>
    </dialog>
  )
}

/** A question with two answers. The confirming button has the focus, so Enter confirms and Escape cancels. */
export function ConfirmDialog({
  open,
  title,
  confirmLabel,
  cancelLabel = 'Cancel',
  busy = false,
  onConfirm,
  onCancel,
  children,
}: {
  open: boolean
  title: string
  confirmLabel: string
  cancelLabel?: string
  busy?: boolean
  onConfirm: () => void
  onCancel: () => void
  children: ReactNode
}) {
  return (
    <Modal open={open} title={title} onClose={onCancel}>
      <div className="space-y-2 text-sm">{children}</div>
      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={onCancel} disabled={busy}>
          {cancelLabel}
        </Button>
        <Button onClick={onConfirm} disabled={busy} autoFocus>
          {confirmLabel}
        </Button>
      </div>
    </Modal>
  )
}
