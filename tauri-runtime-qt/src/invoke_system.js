// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

// Invoke system for the Qt runtime, derived from tauri's ipc-protocol.js.
//
// QtWebEngine does not expose custom-protocol request bodies to embedders
// (`QWebEngineUrlRequestJob::requestBody()` reports an empty, already-ended
// device for POSTs), so the default invoke system - which puts the arguments in
// the request body - cannot work. This script keeps the custom-protocol IPC path
// but carries the payload in the `X-Tauriqt-Payload` header as base64, and the
// runtime turns that header back into the request body before handing the
// request to tauri's IPC handler.
//
// Limitation: the payload size is bounded by the web engine's request-header
// limit (Chromium ~256 KB per request); large/channel payloads need chunking.

;(function () {
  const __TAURI_INVOKE_KEY__ = __INVOKE_KEY__

  const processIpcMessage = function (message) {
    if (
      message instanceof ArrayBuffer
      || ArrayBuffer.isView(message)
      || Array.isArray(message)
    ) {
      return {
        contentType: 'application/octet-stream',
        data: message
      }
    } else {
      const data = JSON.stringify(message, (_k, val) => {
        const SERIALIZE_TO_IPC_FN = '__TAURI_TO_IPC_KEY__'

        if (val instanceof Map) {
          return Object.fromEntries(val.entries())
        } else if (val instanceof Uint8Array) {
          return Array.from(val)
        } else if (val instanceof ArrayBuffer) {
          return Array.from(new Uint8Array(val))
        } else if (
          typeof val === 'object'
          && val !== null
          && SERIALIZE_TO_IPC_FN in val
        ) {
          return val[SERIALIZE_TO_IPC_FN]()
        } else {
          return val
        }
      })

      return {
        contentType: 'application/json',
        data
      }
    }
  }

  function toBase64(data) {
    let bytes
    if (typeof data === 'string') {
      bytes = new TextEncoder().encode(data)
    } else if (data instanceof Uint8Array) {
      bytes = data
    } else {
      bytes = new Uint8Array(data)
    }

    let binary = ''
    const chunk = 0x8000
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk))
    }
    return btoa(binary)
  }

  function sendIpcMessage(message) {
    const { cmd, callback, error, payload, options } = message
    const { contentType, data } = processIpcMessage(payload)

    const headers = new Headers((options && options.headers) || {})
    headers.set('Content-Type', contentType)
    headers.set('Tauri-Callback', callback)
    headers.set('Tauri-Error', error)
    headers.set('Tauri-Invoke-Key', __TAURI_INVOKE_KEY__)
    headers.set('X-Tauriqt-Payload', toBase64(data))

    fetch(window.__TAURI_INTERNALS__.convertFileSrc(cmd, 'ipc'), {
      method: 'POST',
      headers
    })
      .then((response) => {
        const callbackId =
          response.headers.get('Tauri-Response') === 'ok' ? callback : error
        switch ((response.headers.get('content-type') || '').split(',')[0]) {
          case 'application/json':
            return response.json().then((r) => [callbackId, r])
          case 'text/plain':
            return response.text().then((r) => [callbackId, r])
          default:
            return response.arrayBuffer().then((r) => [callbackId, r])
        }
      })
      .then(
        ([callbackId, data]) => {
          window.__TAURI_INTERNALS__.runCallback(callbackId, data)
        },
        (e) => {
          console.error('IPC failed', e)
        }
      )
  }

  Object.defineProperty(window.__TAURI_INTERNALS__, 'postMessage', {
    value: sendIpcMessage
  })
})()
