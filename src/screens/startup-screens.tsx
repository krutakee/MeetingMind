export function LoadingScreen() {
  return (
    <main className="flex h-full items-center justify-center">
      <p role="status" className="text-muted-foreground">
        Starting…
      </p>
    </main>
  )
}

/**
 * Shown instead of the normal screens when the app cannot work: the backend is unreachable, or the database
 * could not be opened (for example, it was created by a newer version). Never closes silently.
 */
export function StartupFailedScreen({
  title,
  message,
  logDir,
}: {
  title: string
  message: string
  logDir?: string
}) {
  return (
    <main className="mx-auto flex h-full max-w-xl flex-col justify-center gap-4 p-8">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <p role="alert" className="rounded-md border-2 border-destructive p-4 text-destructive">
        {message}
      </p>
      {logDir ? (
        <p className="text-sm text-muted-foreground">
          Details are in the log folder: <span className="break-all font-mono">{logDir}</span>
        </p>
      ) : null}
    </main>
  )
}
