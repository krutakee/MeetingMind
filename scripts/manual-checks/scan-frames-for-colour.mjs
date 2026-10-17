// Looks for a solid colour inside one rectangle of every frame of a video. Used to check that a window that should be
// hidden from the recording (the control window, painted solid green for the check) never shows up in it.
//
//   node scan-frames-for-colour.mjs <ffmpeg> <video> <x> <y> <width> <height> [r g b [tolerance]]
//   node scan-frames-for-colour.mjs <ffmpeg> --self-test
//
// A frame "has" the colour when at least 1 % of the pixels in the rectangle are within `tolerance` (default 40, per
// channel) of it. Prints one line of JSON; exits 1 if any frame has the colour, 0 if none does.
// --self-test makes a video that does contain the colour and checks that this scanner finds it (a scanner that never
// finds anything would make the real check pass for nothing).

import { spawn, spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'

const [ffmpeg, ...rest] = process.argv.slice(2)
if (!ffmpeg) {
  console.error('usage: scan-frames-for-colour.mjs <ffmpeg> <video> <x> <y> <width> <height> [r g b [tolerance]]')
  process.exit(2)
}

const even = (n) => Math.max(2, Math.floor(n / 2) * 2)

/** Scans the rectangle of every frame; resolves with what was found. */
function scan(video, rect, colour, tolerance) {
  const [x, y, w, h] = [even(rect[0]), even(rect[1]), even(rect[2]), even(rect[3])]
  return new Promise((resolve, reject) => {
    const child = spawn(
      ffmpeg,
      ['-hide_banner', '-loglevel', 'error', '-i', video, '-an', '-fps_mode', 'passthrough', '-vf', `crop=${w}:${h}:${x}:${y},format=rgb24`, '-f', 'rawvideo', '-'],
      { stdio: ['ignore', 'pipe', 'inherit'] },
    )
    const frameBytes = w * h * 3
    let pending = Buffer.alloc(0)
    let frames = 0
    let framesWithColour = 0
    let maxFraction = 0
    child.stdout.on('data', (chunk) => {
      pending = Buffer.concat([pending, chunk])
      while (pending.length >= frameBytes) {
        let matching = 0
        for (let i = 0; i < frameBytes; i += 3) {
          if (
            Math.abs(pending[i] - colour[0]) <= tolerance &&
            Math.abs(pending[i + 1] - colour[1]) <= tolerance &&
            Math.abs(pending[i + 2] - colour[2]) <= tolerance
          ) {
            matching += 1
          }
        }
        const fraction = matching / (w * h)
        maxFraction = Math.max(maxFraction, fraction)
        if (fraction >= 0.01) framesWithColour += 1
        frames += 1
        pending = pending.subarray(frameBytes)
      }
    })
    child.on('error', reject)
    child.on('close', (code) =>
      code === 0 ? resolve({ frames, framesWithColour, maxFraction: Number(maxFraction.toFixed(4)) }) : reject(new Error(`ffmpeg exited with ${code}`)),
    )
  })
}

if (rest[0] === '--self-test') {
  // A 1 s video, 320x240, solid green. Encoded with whichever H.264 encoder this build has.
  const dir = mkdtempSync(path.join(tmpdir(), 'scan-self-test-'))
  const video = path.join(dir, 'green.mp4')
  let made = false
  for (const codec of ['libopenh264', 'h264_mf']) {
    const result = spawnSync(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=0x00ff00:s=320x240:r=15:d=1', '-c:v', codec, '-pix_fmt', 'yuv420p', video])
    if (result.status === 0) {
      made = true
      break
    }
  }
  if (!made) {
    console.error('self-test: could not make a test video')
    process.exit(2)
  }
  const found = await scan(video, [40, 40, 100, 100], [0, 255, 0], 40)
  const nothing = await scan(video, [40, 40, 100, 100], [255, 0, 255], 40)
  rmSync(dir, { recursive: true, force: true })
  console.log(JSON.stringify({ selfTest: true, green: found, magenta: nothing }))
  const ok = found.frames > 0 && found.framesWithColour === found.frames && nothing.framesWithColour === 0
  console.log(ok ? 'self-test passed: the scanner finds the colour when it is there and not when it is not' : 'self-test FAILED')
  process.exit(ok ? 0 : 1)
}

const [video, x, y, w, h, r = '0', g = '255', b = '0', tolerance = '40'] = rest
const result = await scan(video, [x, y, w, h].map(Number), [r, g, b].map(Number), Number(tolerance))
console.log(JSON.stringify(result))
process.exit(result.framesWithColour > 0 ? 1 : 0)
