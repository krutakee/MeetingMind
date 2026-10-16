// Downloads the pinned LGPL FFmpeg build and installs ffmpeg.exe as a Tauri sidecar (Windows only for now).
//   node scripts/fetch-ffmpeg.mjs
// Result: src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe and FFMPEG-LICENSE.txt (both git-ignored).
// The zip and the executable are both checked against the SHA-256 values below; any mismatch stops the script.
// To move to a newer build: change the PIN, run this script, and re-check the license flags with the app's guard
// (`cargo test` includes a test that rejects any build configured with --enable-gpl or --enable-nonfree).
//
// Note: BtbN keeps dated releases only for a limited time. If the download fails with 404, the pinned release has
// expired and a copy we control is needed (see docs/phase-2-spikes.md).

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'

const PIN = {
  tag: 'autobuild-2026-10-16-13-04',
  asset: 'ffmpeg-n9.0.2-28-g669f5ab083-win64-lgpl-9.0.zip',
  zipSha256: 'fe7aab7fb1e83b4be657ae46d949a01812cabc60d35766d51fad8dcf97c69319',
  exeSha256: 'c47e9873edf44d2df1af10c7f509ec9d79626d173ddcda47a2d2a963d22ff70a',
  innerDir: 'ffmpeg-n9.0.2-28-g669f5ab083-win64-lgpl-9.0',
}

const root = path.resolve(import.meta.dirname, '..')
const binaries = path.join(root, 'src-tauri', 'binaries')
const target = path.join(binaries, 'ffmpeg-x86_64-pc-windows-msvc.exe')
const licenseTarget = path.join(binaries, 'FFMPEG-LICENSE.txt')

function sha256(file) {
  return createHash('sha256').update(readFileSync(file)).digest('hex')
}

if (process.platform !== 'win32') {
  console.log('fetch-ffmpeg: only the Windows build is provisioned for now (macOS arrives in Phase 5). Nothing to do.')
  process.exit(0)
}

if (existsSync(target) && sha256(target) === PIN.exeSha256 && existsSync(licenseTarget)) {
  console.log(`fetch-ffmpeg: ${path.relative(root, target)} is already the pinned build.`)
  process.exit(0)
}

const work = mkdtempSync(path.join(tmpdir(), 'meetingmind-ffmpeg-'))
try {
  const url = `https://github.com/BtbN/FFmpeg-Builds/releases/download/${PIN.tag}/${PIN.asset}`
  const zip = path.join(work, PIN.asset)
  console.log(`fetch-ffmpeg: downloading ${url}`)
  const response = await fetch(url, { redirect: 'follow' })
  if (!response.ok || !response.body) {
    throw new Error(`download failed: HTTP ${response.status}. The pinned release may have expired.`)
  }
  await pipeline(Readable.fromWeb(response.body), createWriteStream(zip))

  const zipHash = sha256(zip)
  if (zipHash !== PIN.zipSha256) {
    throw new Error(`zip checksum mismatch.\n  expected ${PIN.zipSha256}\n  got      ${zipHash}`)
  }

  // bsdtar (built into Windows 10+) extracts single members from a zip.
  execFileSync('tar', ['-xf', zip, '-C', work, `${PIN.innerDir}/bin/ffmpeg.exe`, `${PIN.innerDir}/LICENSE.txt`], {
    stdio: 'inherit',
  })
  const exe = path.join(work, PIN.innerDir, 'bin', 'ffmpeg.exe')
  const exeHash = sha256(exe)
  if (exeHash !== PIN.exeSha256) {
    throw new Error(`ffmpeg.exe checksum mismatch.\n  expected ${PIN.exeSha256}\n  got      ${exeHash}`)
  }

  mkdirSync(binaries, { recursive: true })
  copyFileSync(exe, target)
  copyFileSync(path.join(work, PIN.innerDir, 'LICENSE.txt'), licenseTarget)
  console.log(`fetch-ffmpeg: installed ${path.relative(root, target)} (checksums verified).`)
} finally {
  rmSync(work, { recursive: true, force: true })
}
