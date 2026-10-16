/** A screen that exists in the navigation but has no function yet. It says so plainly; nothing here is simulated. */
export function NotImplementedScreen({
  title,
  phase,
  description,
}: {
  title: string
  phase: string
  description: string
}) {
  return (
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <section
        aria-label="Status"
        className="mt-6 rounded-md border border-border bg-card p-6"
      >
        <p className="font-medium">Not yet implemented</p>
        <p className="mt-2 text-muted-foreground">{description}</p>
        <p className="mt-2 text-sm text-muted-foreground">Planned for {phase}.</p>
      </section>
    </div>
  )
}
