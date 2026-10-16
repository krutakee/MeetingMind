// Regenerates LICENSES.md from the real dependency data:
//   - Rust: `cargo metadata` for the two supported platforms (Windows and macOS), using Cargo.lock
//   - JavaScript: `pnpm licenses list` for what is installed
// Run it with `pnpm licenses:generate` after adding or updating any dependency, and commit the result.
// Licenses are the ones the packages declare in their metadata; the script does not read license texts.

import { execSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const RUST_TARGETS = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin']

// What each direct dependency is used for. A direct dependency missing here makes the script fail, so the
// table in LICENSES.md can never contain an unexplained dependency.
const PURPOSE = {
  // Rust
  tauri: 'Desktop app framework (window, webview, IPC)',
  'tauri-build': 'Build step for Tauri',
  'tauri-plugin-dialog': 'Native folder picker',
  rusqlite: 'SQLite access; bundles SQLite with FTS5',
  serde: 'Data (de)serialization',
  serde_json: 'JSON for settings and the UI bridge',
  sysinfo: 'RAM and CPU detection',
  thiserror: 'Error types',
  tracing: 'Structured logging API',
  'tracing-appender': 'Rotating log files',
  'tracing-subscriber': 'Log formatting and filtering',
  tempfile: 'Temporary folders in tests',
  // JavaScript
  react: 'UI library',
  'react-dom': 'UI rendering',
  'react-router-dom': 'Screen navigation',
  '@tauri-apps/api': 'Calls from the UI to the Rust backend',
  '@tauri-apps/plugin-dialog': 'Folder picker from the UI',
  '@radix-ui/react-label': 'Accessible label component',
  '@radix-ui/react-radio-group': 'Accessible radio group component',
  '@radix-ui/react-switch': 'Accessible switch component',
  'class-variance-authority': 'Component style variants',
  clsx: 'Class name helper',
  'lucide-react': 'Icons',
  'tailwind-merge': 'Resolves conflicting Tailwind classes',
  vite: 'Frontend dev server and bundler',
  '@vitejs/plugin-react': 'React support in Vite',
  typescript: 'Type checking',
  tailwindcss: 'CSS framework',
  '@tailwindcss/vite': 'Tailwind support in Vite',
  '@tauri-apps/cli': 'Tauri build and dev commands',
  vitest: 'Frontend test runner',
  jsdom: 'Browser simulation for frontend tests',
  '@testing-library/react': 'Component testing helpers',
  '@testing-library/jest-dom': 'Test assertions for the DOM',
  '@testing-library/user-event': 'Simulated user input in tests',
  '@types/react': 'Type definitions',
  '@types/react-dom': 'Type definitions',
  '@types/node': 'Type definitions',
  eslint: 'Linting',
  '@eslint/js': 'Lint rules',
  'typescript-eslint': 'Lint rules for TypeScript',
  'eslint-plugin-react-hooks': 'Lint rules for React hooks',
  globals: 'Lint environment definitions',
}

// SPDX identifiers treated as permissive: they allow commercial use and redistribution with only
// notice/attribution duties. Everything else is listed under "Needs review".
const PERMISSIVE = new Set([
  'MIT',
  'MIT-0',
  'Apache-2.0',
  'Apache-2.0 WITH LLVM-exception',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'ISC',
  '0BSD',
  'Unlicense',
  'Zlib',
  'CC0-1.0',
  'BSL-1.0',
  'BlueOak-1.0.0',
  'Unicode-3.0',
  'Unicode-DFS-2016',
  'CDLA-Permissive-2.0',
])

function run(command, cwd) {
  return execSync(command, { cwd, encoding: 'utf8', maxBuffer: 512 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'] })
}

/** True if the license expression can be satisfied using only permissive licenses (OR = choose one, AND = all). */
function isPermissive(expression) {
  const tokens = expression
    .replace(/\//g, ' OR ')
    .replace(/[()]/g, (paren) => ` ${paren} `)
    .split(/\s+/)
    .filter(Boolean)
  let position = 0
  function atom() {
    if (tokens[position] === '(') {
      position++
      const value = orExpr()
      position++ // ")"
      return value
    }
    let id = tokens[position++]
    if (tokens[position] === 'WITH') {
      position++
      id += ` WITH ${tokens[position++]}`
    }
    return PERMISSIVE.has(id)
  }
  function andExpr() {
    let value = atom()
    while (tokens[position] === 'AND') {
      position++
      const next = atom()
      value = value && next
    }
    return value
  }
  function orExpr() {
    let value = andExpr()
    while (tokens[position] === 'OR') {
      position++
      const next = andExpr()
      value = value || next
    }
    return value
  }
  return orExpr()
}

const cell = (text) => String(text).replace(/\|/g, '\\|')
const table = (header, rows) =>
  [`| ${header.join(' | ')} |`, `| ${header.map(() => '---').join(' | ')} |`, ...rows.map((row) => `| ${row.map(cell).join(' | ')} |`)].join('\n')

// ---------- Rust ----------
const rustPackages = new Map() // "name version" -> { name, version, license }
let direct = null
for (const target of RUST_TARGETS) {
  const metadata = JSON.parse(
    run(`cargo metadata --format-version 1 --locked --filter-platform ${target}`, path.join(root, 'src-tauri')),
  )
  const byId = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]))
  const rootId = metadata.resolve.root
  const used = new Set(metadata.resolve.nodes.map((node) => node.id))
  for (const id of used) {
    if (id === rootId) continue
    const pkg = byId.get(id)
    if (!pkg) continue
    const license = pkg.license ?? (pkg.license_file ? `(see ${pkg.license_file})` : 'MISSING')
    rustPackages.set(`${pkg.name} ${pkg.version}`, { name: pkg.name, version: pkg.version, license })
  }
  if (!direct) {
    const rootNode = metadata.resolve.nodes.find((node) => node.id === rootId)
    const rootPackage = byId.get(rootId)
    // The package lists itself as a dev-dependency (to switch on its test-only feature); that is not a third party.
    direct = rootPackage.dependencies.filter((dependency) => dependency.name !== rootPackage.name).map((dependency) => {
      const kind = dependency.kind ?? 'normal'
      const resolved = rootNode.deps.find((dep) => byId.get(dep.pkg)?.name === dependency.name)
      const pkg = byId.get(resolved.pkg)
      return {
        name: pkg.name,
        version: pkg.version,
        kind: kind === 'normal' ? 'runtime' : kind,
        license: pkg.license ?? 'MISSING',
      }
    })
  }
}
const rustDirect = direct.sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name))

// ---------- JavaScript ----------
const packageJson = JSON.parse(run('node -p "JSON.stringify(require(\'./package.json\'))"', root))
const jsLicenseData = JSON.parse(run('pnpm licenses list --json', root))
const jsPackages = []
for (const [license, entries] of Object.entries(jsLicenseData)) {
  for (const entry of entries) {
    for (const version of entry.versions) jsPackages.push({ name: entry.name, version, license })
  }
}
const jsDirect = [
  ...Object.keys(packageJson.dependencies ?? {}).map((name) => ({ name, kind: 'runtime' })),
  ...Object.keys(packageJson.devDependencies ?? {}).map((name) => ({ name, kind: 'dev' })),
]
  .map((dependency) => {
    const found = jsPackages.find((pkg) => pkg.name === dependency.name)
    if (!found) throw new Error(`pnpm licenses did not report ${dependency.name}`)
    return { ...dependency, version: found.version, license: found.license }
  })
  .sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name))

for (const dependency of [...rustDirect, ...jsDirect]) {
  if (!PURPOSE[dependency.name]) throw new Error(`Add a PURPOSE entry for ${dependency.name} in scripts/generate-licenses.mjs`)
}

// ---------- Summaries ----------
function countBy(items) {
  const counts = new Map()
  for (const item of items) counts.set(item.license, (counts.get(item.license) ?? 0) + 1)
  return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
}
const rustAll = [...rustPackages.values()]
const needsReview = (items) =>
  items.filter((item) => item.license === 'MISSING' || !isPermissive(item.license)).sort((a, b) => a.name.localeCompare(b.name))
const rustReview = needsReview(rustAll)
const jsReview = needsReview(jsPackages)

const reviewTable = (items) =>
  items.length
    ? table(['Package', 'Version', 'Declared license'], items.map((item) => [item.name, item.version, item.license]))
    : '_None. Every package declares a license that can be satisfied with a permissive one._'

const markdown = `# Third-party licenses

Generated by \`pnpm licenses:generate\` (\`scripts/generate-licenses.mjs\`). Do not edit by hand: change a dependency, run the
command, commit the result. CLAUDE.md requires this record for every dependency.

**What this is.** The license each package *declares in its metadata* (Rust: \`cargo metadata\` from \`Cargo.lock\`, for
Windows and macOS; JavaScript: \`pnpm licenses\`). Licenses written as "A OR B" let the user choose one; the check
below counts a package as permissive when at least one permitted choice is. License texts were not read and this is
not legal advice. Get a legal review before distributing MeetingMind commercially.

**Summary.** ${rustAll.length} Rust crates and ${jsPackages.length} JavaScript packages are in the dependency trees.
${rustReview.length + jsReview.length === 0 ? 'All of them declare permissive licenses (MIT, Apache-2.0, BSD, ISC, Zlib and similar).' : `${rustReview.length} Rust crate(s) and ${jsReview.length} JavaScript package(s) need review (listed below); all others declare permissive licenses.`}

## Direct dependencies

### Rust (\`src-tauri/Cargo.toml\`)

${table(['Crate', 'Version', 'Kind', 'License', 'Used for'], rustDirect.map((d) => [d.name, d.version, d.kind, d.license, PURPOSE[d.name]]))}

### JavaScript (\`package.json\`)

${table(['Package', 'Version', 'Kind', 'License', 'Used for'], jsDirect.map((d) => [d.name, d.version, d.kind, d.license, PURPOSE[d.name]]))}

"runtime" and "build" items end up in, or build, the shipped app; "dev" items are only used while developing and testing.

## Whole dependency trees

### Rust: licenses declared by all ${rustAll.length} crates (Windows + macOS)

${table(['Declared license', 'Crates'], countBy(rustAll))}

### JavaScript: licenses declared by all ${jsPackages.length} packages

${table(['Declared license', 'Packages'], countBy(jsPackages))}

## Needs review

### Rust

${reviewTable(rustReview)}

### JavaScript

${reviewTable(jsReview)}

Why these are listed: MPL-2.0 is a file-level copyleft license. It can be used inside a proprietary app; the duties
attach to the MPL-licensed files themselves (keep their notices, and share any changes made to those files).
CC-BY-4.0 asks for attribution. The Rust items are in the dependency tree of the app (mostly pulled in by Tauri). The
JavaScript items arrive through development tooling (Tailwind, Vite, ESLint) and are not part of the shipped bundle.

## Components that are not packages

| Component | Terms | Notes |
| --- | --- | --- |
| SQLite | Public domain | Compiled into the app by \`rusqlite\` (\`bundled\` feature), with FTS5. The \`rusqlite\` and \`libsqlite3-sys\` crates are listed above under their own licenses. |
| Microsoft Edge WebView2 Runtime | Microsoft software license | Not part of this repository and not bundled by it. The app window needs it on Windows. Check how the Tauri installer provides it (\`webviewInstallMode\`) before distributing, because that decides whether the installer contacts Microsoft at install time. |
| NSIS 3.13 | zlib/libpng for the program and the zlib module; bzip2 license for the bzip2 module; Common Public License 1.0 for the LZMA module | Downloaded by Tauri to build the Windows installer (read from the \`COPYING\` file in the downloaded tool). Parts of NSIS end up inside the installer it produces. Have the LZMA/CPL 1.0 term reviewed before distributing the Windows installer. |
`

writeFileSync(path.join(root, 'LICENSES.md'), markdown)
console.log(
  `LICENSES.md written: ${rustAll.length} Rust crates, ${jsPackages.length} JS packages, ` +
    `${rustReview.length + jsReview.length} need review`,
)
