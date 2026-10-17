// A tiny Chrome DevTools Protocol client for the app's own WebView2 pages. Start the app with the environment variable
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9347
// (a port nothing else uses; CDP_PORT says which). Only pages that are MeetingMind's own are ever touched: another
// program may be listening on a debugging port too, and its pages are left alone.
// Usage: node cdp.mjs <label: main|control> shot <file.png> [width height]
//        node cdp.mjs <label> eval "<javascript expression>"
import { writeFileSync } from 'node:fs'

const [label, command, ...rest] = process.argv.slice(2)
const port = Number(process.env.CDP_PORT ?? 9347)

async function pages() {
  const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
  // The app's pages come from the Vite dev server or from the app itself; anything else belongs to another program.
  const ours = /^(http:\/\/localhost:1420|http:\/\/tauri\.localhost|tauri:\/\/localhost)/
  return list.filter((t) => t.type === 'page' && ours.test(t.url))
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url)
    let nextId = 1
    const waiting = new Map()
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data)
      if (message.id && waiting.has(message.id)) {
        const { resolve: ok, reject: fail } = waiting.get(message.id)
        waiting.delete(message.id)
        if (message.error) fail(new Error(JSON.stringify(message.error)))
        else ok(message.result)
      }
    })
    socket.addEventListener('error', reject)
    socket.addEventListener('open', () =>
      resolve({
        send: (method, params = {}) =>
          new Promise((ok, fail) => {
            const id = nextId++
            waiting.set(id, { resolve: ok, reject: fail })
            socket.send(JSON.stringify({ id, method, params }))
          }),
        close: () => socket.close(),
      }),
    )
  })
}

async function windowLabel(client) {
  const result = await client.send('Runtime.evaluate', {
    expression: 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? "unknown"',
    returnByValue: true,
  })
  return result.result.value
}

async function pageFor(wanted) {
  for (const target of await pages()) {
    const client = await connect(target.webSocketDebuggerUrl)
    if ((await windowLabel(client)) === wanted) return client
    client.close()
  }
  throw new Error(`no page with window label "${wanted}"`)
}

const client = await pageFor(label)
try {
  if (command === 'shot') {
    const [file, width, height] = rest
    if (width && height) {
      await client.send('Emulation.setDeviceMetricsOverride', { width: Number(width), height: Number(height), deviceScaleFactor: 1, mobile: false })
    }
    const { data } = await client.send('Page.captureScreenshot', { format: 'png' })
    writeFileSync(file, Buffer.from(data, 'base64'))
    console.log('saved', file)
  } else if (command === 'eval') {
    const result = await client.send('Runtime.evaluate', { expression: rest.join(' '), awaitPromise: true, returnByValue: true })
    if (result.exceptionDetails) console.log('EXCEPTION', JSON.stringify(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text))
    else console.log(JSON.stringify(result.result.value))
  } else {
    console.log('unknown command', command)
  }
} finally {
  client.close()
}
