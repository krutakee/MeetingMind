import { NavLink, Outlet } from 'react-router-dom'
import { Library, Mic, Settings as SettingsIcon, type LucideIcon } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useAppState } from '@/state/app-state'

const NAV_ITEMS: { to: string; label: string; icon: LucideIcon }[] = [
  { to: '/record', label: 'Record', icon: Mic },
  { to: '/library', label: 'Library', icon: Library },
  { to: '/settings', label: 'Settings', icon: SettingsIcon },
]

export function Layout() {
  const { info } = useAppState()
  return (
    <div className="flex h-full">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-10 focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-foreground"
      >
        Skip to content
      </a>
      <aside className="flex w-52 shrink-0 flex-col gap-6 border-r border-border bg-card p-4">
        <p className="text-lg font-semibold">{info.productName}</p>
        <nav aria-label="Main">
          <ul className="flex flex-col gap-1">
            {NAV_ITEMS.map(({ to, label, icon: Icon }) => (
              <li key={to}>
                <NavLink
                  to={to}
                  className={({ isActive }) =>
                    cn(
                      'flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium',
                      'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
                      isActive ? 'bg-primary text-primary-foreground' : 'hover:bg-muted',
                    )
                  }
                >
                  <Icon aria-hidden="true" className="size-4" />
                  {label}
                </NavLink>
              </li>
            ))}
          </ul>
        </nav>
      </aside>
      <main id="main" tabIndex={-1} className="min-w-0 flex-1 overflow-y-auto p-8">
        <Outlet />
      </main>
    </div>
  )
}
