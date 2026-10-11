// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT
//
// Tiny CDP client helpers shared by tools/cdp-probe/{probe,mitm}.mjs.
// No npm dependencies: Node's built-in global `WebSocket` and `fetch`
// (Node >= 22; verified on v24). Node's WebSocket sends no `Origin` header,
// which matters because Chromium's DevTools server rejects non-allowlisted
// origins when one is present.

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const BASE64_RE = /^[A-Za-z0-9+/]+={0,2}$/;

/** Long strings that are pure base64 (screenshots, PDFs) get folded, not sliced. */
export function isLikelyBase64(value) {
  return value.length > 256 && BASE64_RE.test(value);
}

/**
 * Fold a base64 PNG into a descriptor that keeps the useful facts and drops the
 * pixels: `{chars, approxBytes, format:'png', width, height, head, tail}`.
 * Width/height come from the IHDR chunk (bytes 16..24).
 */
export function foldPng(base64) {
  const out = {
    '@base64': true,
    chars: base64.length,
    approxBytes: Math.floor((base64.length * 3) / 4),
    head: base64.slice(0, 24),
    tail: base64.slice(-8),
  };
  try {
    const buf = Buffer.from(base64.slice(0, 64), 'base64');
    if (buf.length >= 24 && buf.readUInt32BE(0) === 0x89504e47 && buf.toString('latin1', 12, 16) === 'IHDR') {
      out.format = 'png';
      out.width = buf.readUInt32BE(16);
      out.height = buf.readUInt32BE(20);
    }
  } catch {}
  return out;
}

/**
 * Structural, JSON-safe copy of a CDP payload with long strings, long arrays
 * and deep nesting folded into annotated placeholders. `shrink` never throws
 * and never mutates its input.
 */
export function shrink(value, opts = {}) {
  const { maxString = 400, maxItems = 50, maxDepth = 8, depth = 0 } = opts;
  if (typeof value === 'string') {
    if (value.length <= maxString) return value;
    if (isLikelyBase64(value)) {
      if (value.startsWith('iVBORw0KGgo')) return foldPng(value);
      return {
        '@base64': true,
        chars: value.length,
        approxBytes: Math.floor((value.length * 3) / 4),
        head: value.slice(0, 24),
        tail: value.slice(-8),
      };
    }
    return `${value.slice(0, maxString)}…(+${value.length - maxString} chars)`;
  }
  if (Array.isArray(value)) {
    const out = value.slice(0, maxItems).map((v) => shrink(v, { ...opts, depth: depth + 1 }));
    if (value.length > maxItems) out.push({ '@moreItems': value.length - maxItems });
    return out;
  }
  if (value && typeof value === 'object') {
    if (depth >= maxDepth) return { '@folded': Object.keys(value).join(',') || 'object' };
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = shrink(v, { ...opts, depth: depth + 1 });
    return out;
  }
  return value;
}

/** One-line human summary of a probe response, for the compact table. */
export function summarize(method, entry) {
  if (entry.status === 'timeout') return `no response (timeout ${entry.timeoutMs} ms)`;
  if (entry.status === 'skipped') return `skipped (${entry.skipReason})`;
  if (entry.status === 'connection-closed') return 'not run (connection closed)';
  if (entry.status === 'error') {
    const err = entry.error ?? {};
    return `${err.code}: ${err.message ?? ''}`;
  }
  const r = entry.result ?? {};
  switch (method) {
    case 'Browser.getVersion':
      return `${r.product ?? '?'} protocol=${r.protocolVersion ?? '?'} js=${r.jsVersion ?? '?'}`;
    case 'Runtime.evaluate': {
      const res = r.result ?? {};
      if (res.type === 'undefined') return 'undefined';
      return `${res.type}=${JSON.stringify(res.value)?.slice(0, 60)}`;
    }
    case 'Page.captureScreenshot':
    case 'Page.printToPDF': {
      const d = r.data ?? {};
      const dims = d.width ? ` ${d.width}x${d.height}` : '';
      return `base64 ${d.chars ?? 0} chars (~${d.approxBytes ?? 0} B)${dims}, head=${d.head ?? '?'}`;
    }
    case 'Target.getTargets':
      return `targetInfos=${r.targetInfos?.length ?? 0}`;
    case 'Target.getBrowserContexts':
      return `browserContextIds=${JSON.stringify(r.browserContextIds)?.slice(0, 40)}`;
    case 'Network.getAllCookies':
    case 'Network.getCookies':
    case 'Storage.getCookies':
      return `cookies=${r.cookies?.length ?? 0}`;
    case 'Network.setCookie':
      return `success=${r.success}`;
    case 'Target.attachToTarget':
      return `sessionId=${JSON.stringify(r.sessionId)}`;
    case 'DOM.getDocument':
      return `root=${r.root?.nodeName ?? '?'} childNodeCount=${r.root?.childNodeCount ?? '?'}`;
    case 'DOM.querySelector':
      return `nodeId=${r.nodeId}`;
    case 'Page.getFrameTree': {
      const f = r.frameTree?.frame ?? {};
      return `frameId=${f.id} url=${f.url ?? '?'}`;
    }
    case 'Network.getResponseBody':
      return `bodyLen=${r.body?.length ?? 0} base64Encoded=${r.base64Encoded}`;
    default: {
      const s = JSON.stringify(r);
      return s.length > 90 ? `${s.slice(0, 90)}…` : s;
    }
  }
}

/** Minimal CDP client over Node's global WebSocket. One command at a time. */
export class CdpClient {
  /** @param {string} url ws:// endpoint @param {{connectTimeoutMs?: number}} opts */
  constructor(url, { connectTimeoutMs = 10_000 } = {}) {
    this.url = url;
    this.connectTimeoutMs = connectTimeoutMs;
    this.events = []; // {method, sessionId?, params (shrunk), atMs, rawBytes}
    this.pending = new Map();
    this.nextId = 1;
    this.closed = false;
    this.ws = null;
    this.startedAt = Date.now();
  }

  at() {
    return Date.now() - this.startedAt;
  }

  connect() {
    return new Promise((resolve, reject) => {
      let settled = false;
      const ws = new WebSocket(this.url);
      this.ws = ws;
      const timer = setTimeout(() => {
        if (settled) return;
        settled = true;
        try {
          ws.close();
        } catch {}
        reject(new Error(`websocket connect timed out after ${this.connectTimeoutMs} ms (${this.url})`));
      }, this.connectTimeoutMs);
      ws.addEventListener('open', () => {
        clearTimeout(timer);
        if (settled) return;
        settled = true;
        resolve();
      });
      ws.addEventListener('error', (e) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        reject(new Error(`websocket error: ${e?.message || 'unknown'}`));
      });
      ws.addEventListener('message', (e) => this.#onMessage(String(e.data)));
      ws.addEventListener('close', (e) => this.#onClose(e.code, e.reason));
    });
  }

  #onClose(code, reason) {
    this.closed = true;
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.resolve({
        method: p.method,
        sessionId: p.sessionId,
        params: p.params,
        status: 'connection-closed',
        timeoutMs: p.timeoutMs,
        durationMs: Date.now() - p.sentAtMs,
      });
    }
    this.pending.clear();
    this.closedInfo = { code, reason: reason || '' };
  }

  #onMessage(text) {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      this.events.push({ method: '<unparsable>', raw: text.slice(0, 200), atMs: this.at() });
      return;
    }
    if (msg.id !== undefined && msg.id !== null) {
      const p = this.pending.get(msg.id);
      if (!p) return; // late response to a command we already timed out
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      p.resolve({
        method: p.method,
        sessionId: p.sessionId,
        params: p.params,
        status: msg.error ? 'error' : 'result',
        timeoutMs: p.timeoutMs,
        durationMs: Date.now() - p.sentAtMs,
        rawBytes: text.length,
        ...(msg.error ? { error: msg.error } : { result: p.raw ? msg.result : shrink(msg.result) }),
      });
      return;
    }
    if (msg.method) {
      this.events.push({
        method: msg.method,
        sessionId: msg.sessionId,
        params: shrink(msg.params, { maxString: 500 }),
        atMs: this.at(),
        rawBytes: text.length,
      });
    }
  }

  /**
   * Send one command and resolve with a record describing the outcome:
   * {method, params, status: 'result'|'error'|'timeout'|'connection-closed',
   *  result?, error?, timeoutMs, durationMs, rawBytes?, sessionId?}
   */
  send(method, params = undefined, { timeoutMs = 5_000, sessionId, raw = false } = {}) {
    if (this.closed) {
      return Promise.resolve({ method, sessionId, params, status: 'connection-closed', timeoutMs, durationMs: 0 });
    }
    const id = this.nextId++;
    const sentAtMs = Date.now();
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) {
          resolve({ method, sessionId, params, status: 'timeout', timeoutMs, durationMs: Date.now() - sentAtMs });
        }
      }, timeoutMs);
      this.pending.set(id, { resolve, timer, sentAtMs, method, sessionId, params, timeoutMs, raw });
      const frame = { id, method };
      if (params !== undefined) frame.params = params;
      if (sessionId) frame.sessionId = sessionId;
      try {
        this.ws.send(JSON.stringify(frame));
      } catch (e) {
        clearTimeout(timer);
        this.pending.delete(id);
        resolve({ method, sessionId, params, status: 'error', timeoutMs, durationMs: 0, error: { code: 0, message: `send failed: ${e.message}` } });
      }
    });
  }
}

/** HTTP GET helper for the /json/* devtools endpoints. */
export async function httpJson(host, port, path) {
  const res = await fetch(`http://${host}:${port}${path}`);
  const text = await res.text();
  let body;
  let parseError = null;
  try {
    body = JSON.parse(text);
  } catch (e) {
    parseError = e.message;
    body = text;
  }
  return { path, status: res.status, body, parseError };
}

/** Poll until the devtools HTTP endpoint answers or the deadline passes. */
export async function waitForDevtools(host, port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError = 'no attempt';
  while (Date.now() < deadline) {
    try {
      const r = await httpJson(host, port, '/json/version');
      if (r.status === 200) return r;
      lastError = `HTTP ${r.status}`;
    } catch (e) {
      lastError = e.message;
    }
    await sleep(250);
  }
  throw new Error(`no CDP endpoint on ${host}:${port} after ${timeoutMs} ms (last: ${lastError})`);
}
