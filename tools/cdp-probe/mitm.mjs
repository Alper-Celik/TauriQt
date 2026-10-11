#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT
//
// Transparent (optionally stubbing) CDP proxy used to measure what a client —
// in practice Playwright — actually sends to QtWebEngine's DevTools server, and
// to validate a candidate stub set for the missing commands.
//
//   node tools/cdp-probe/mitm.mjs \
//       --listen 9223 --upstream 127.0.0.1:9222 \
//       --log tools/cdp-probe/last-run-mitm.jsonl [--stubs tools/cdp-probe/stubs.example.json]
//
// Then point Playwright at the proxy instead of Qt:
//   node tools/cdp-probe/playwright-attach.mjs http://127.0.0.1:9223
//
// HTTP: GET /json/version and /json/list are fetched from the upstream and any
// `webSocketDebuggerUrl` is rewritten to the proxy's address, so an
// `http://host:port` endpoint works end to end.
// WS: every upgrade path (/devtools/browser/<id>, /devtools/page/<id>) is
// piped byte-accurately to the same path upstream; every JSON message in both
// directions is appended to the JSONL log as
//   {"dir":"c2s"|"s2c","at":<ms>,"stubbed":<bool>,"msg":{...}}
//
// Stub file shape (all parts optional):
//   {
//     "vars": { "sessionId": "PROXY-SESSION", "targetId": "..." },
//     "requests": {
//       "Browser.setDownloadBehavior": { "result": {} },
//       "Target.getBrowserContexts": { "result": { "browserContextIds": [] } }
//     },
//     "injectEvents": {
//       "Target.setAutoAttach": [
//         { "method": "Target.attachedToTarget",
//           "params": { "sessionId": "$sessionId", "targetInfo": { ... }, "waitingForDebugger": false } }
//       ]
//     }
//   }
// `$name` inside injected events is substituted from "vars". Responses for
// stubbed requests are synthesised locally; everything else is forwarded.
//
// Safe to run repeatedly: nothing is written outside --log; no npm packages.

import { createHash } from 'node:crypto';
import { appendFileSync, mkdirSync, readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, resolve } from 'node:path';
import { CdpClient, shrink } from './cdp-lib.mjs';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

function parseArgs(argv) {
  const args = { listen: 9223, host: '127.0.0.1', upstream: '127.0.0.1:9222', log: null, stubs: null, fullLog: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    if (a === '--listen') args.listen = Number(next());
    else if (a === '--host') args.host = next();
    else if (a === '--upstream') args.upstream = next();
    else if (a === '--log') args.log = resolve(next());
    else if (a === '--stubs') args.stubs = resolve(next());
    else if (a === '--full-log') args.fullLog = true;
    else if (a === '-h' || a === '--help') {
      console.log('usage: mitm.mjs [--listen N] [--host H] [--upstream HOST:PORT] [--log FILE] [--stubs FILE] [--full-log]');
      process.exit(0);
    } else throw new Error(`unknown argument: ${a}`);
  }
  return args;
}

// ---------- minimal WebSocket frame codec (server side) ----------

function encodeText(payload) {
  const data = Buffer.from(payload, 'utf8');
  const len = data.length;
  let header;
  if (len < 126) {
    header = Buffer.alloc(2);
    header[1] = len;
  } else if (len < 65536) {
    header = Buffer.alloc(4);
    header[1] = 126;
    header.writeUInt16BE(len, 2);
  } else {
    header = Buffer.alloc(10);
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(len), 2);
  }
  header[0] = 0x81; // FIN + text
  return Buffer.concat([header, data]);
}

function encodeClose(code = 1000) {
  const b = Buffer.alloc(4);
  b[0] = 0x88;
  b[1] = 2;
  b.writeUInt16BE(code, 2);
  return b;
}

/** Pull as many complete frames as possible out of `buf`. */
function decodeFrames(buf) {
  const frames = [];
  let offset = 0;
  while (offset + 2 <= buf.length) {
    const b0 = buf[offset];
    const b1 = buf[offset + 1];
    const fin = (b0 & 0x80) !== 0;
    const opcode = b0 & 0x0f;
    const masked = (b1 & 0x80) !== 0;
    let len = b1 & 0x7f;
    let p = offset + 2;
    if (len === 126) {
      if (p + 2 > buf.length) break;
      len = buf.readUInt16BE(p);
      p += 2;
    } else if (len === 127) {
      if (p + 8 > buf.length) break;
      const big = buf.readBigUInt64BE(p);
      if (big > 64n * 1024n * 1024n) throw new Error(`frame too large: ${big}`);
      len = Number(big);
      p += 8;
    }
    let mask = null;
    if (masked) {
      if (p + 4 > buf.length) break;
      mask = buf.subarray(p, p + 4);
      p += 4;
    }
    if (p + len > buf.length) break;
    const payload = Buffer.from(buf.subarray(p, p + len));
    if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
    frames.push({ fin, opcode, payload });
    offset = p + len;
  }
  return { frames, rest: buf.subarray(offset) };
}

class ServerSocketChannel {
  constructor(socket, onMessage, onClose) {
    this.socket = socket;
    this.onMessage = onMessage;
    this.onClose = onClose;
    this.buf = Buffer.alloc(0);
    this.fragments = [];
    this.fragmentOpcode = null;
    this.closed = false;
    socket.on('data', (d) => this.#feed(d));
    socket.on('close', () => this.#closed());
    socket.on('error', () => this.#closed());
  }
  #feed(d) {
    this.buf = Buffer.concat([this.buf, d]);
    let decoded;
    try {
      decoded = decodeFrames(this.buf);
    } catch (e) {
      console.error(`mitm: frame decode error: ${e.message}`);
      this.socket.destroy();
      return;
    }
    this.buf = decoded.rest;
    for (const f of decoded.frames) {
      if (f.opcode === 0x8) {
        this.socket.end(encodeClose(1000));
        this.#closed();
        return;
      }
      if (f.opcode === 0x9) continue; // ping: no pong needed for this probe
      if (f.opcode === 0xa) continue; // pong
      if (f.opcode === 0x0) {
        this.fragments.push(f.payload);
      } else {
        this.fragments = [f.payload];
        this.fragmentOpcode = f.opcode;
      }
      if (f.fin && this.fragments.length) {
        const payload = Buffer.concat(this.fragments);
        this.fragments = [];
        const op = this.fragmentOpcode;
        this.fragmentOpcode = null;
        if (op === 0x1 || op === 0x2) this.onMessage(payload.toString('utf8'));
      }
    }
  }
  sendText(text) {
    if (this.closed) return;
    this.socket.write(encodeText(text));
  }
  #closed() {
    if (this.closed) return;
    this.closed = true;
    this.onClose?.();
  }
}

// ---------- proxy ----------

function main() {
  const args = parseArgs(process.argv.slice(2));
  const [upHost, upPort] = args.upstream.split(':');
  const stubs = args.stubs ? JSON.parse(readFileSync(args.stubs, 'utf8')) : {};
  let vars = stubs.vars ?? {};
  let requestStubs = stubs.requests ?? {};
  let injectEvents = stubs.injectEvents ?? {};
  let rewrites = stubs.rewrites ?? {};
  let stubsMtime = 0;
  const reloadStubs = () => {
    if (!args.stubs) return;
    try {
      const { mtimeMs } = statSync(args.stubs);
      if (mtimeMs === stubsMtime) return;
      const fresh = JSON.parse(readFileSync(args.stubs, 'utf8'));
      vars = fresh.vars ?? {};
      requestStubs = fresh.requests ?? {};
      injectEvents = fresh.injectEvents ?? {};
      rewrites = fresh.rewrites ?? {};
      stubsMtime = mtimeMs;
      console.log(`mitm: (re)loaded stubs: ${Object.keys(requestStubs).join(', ') || '(none)'}; rewrites: ${Object.keys(rewrites).join(', ') || '(none)'}`);
    } catch (e) {
      console.error(`mitm: stub reload failed: ${e.message}`);
    }
  };
  reloadStubs();
  if (args.stubs) setInterval(reloadStubs, 1000).unref();

  if (args.log) mkdirSync(dirname(args.log), { recursive: true });
  // Base64 screenshots make a raw log ~1 MB per Playwright run; fold long
  // payloads unless --full-log is passed. Folding never affects proxying.
  const fullLog = args.fullLog;
  const logMsg = (m) => (fullLog ? m : shrink(m, { maxString: 200, maxItems: 40, maxDepth: 6 }));
  const logLine = (obj) => {
    if (args.log) appendFileSync(args.log, `${JSON.stringify({ ...obj, msg: obj.msg === undefined ? undefined : logMsg(obj.msg) })}\n`);
  };

  const subst = (v) => JSON.parse(JSON.stringify(v).replace(/\$([A-Za-z0-9_]+)/g, (m, k) => (k in vars ? String(vars[k]) : m)));

  // Many domains (Network.*, Runtime.*, ...) exist only on a page session: the
  // browser endpoint answers `'Network.enable' wasn't found`. Rewrites marked
  // `on: "page"` therefore run on a dedicated connection to the page endpoint.
  let pageChannel = null;
  let pageChannelConnecting = null;
  const getPageChannel = async () => {
    if (pageChannel && !pageChannel.closed) return pageChannel;
    if (pageChannelConnecting) return pageChannelConnecting;
    pageChannelConnecting = (async () => {
      const list = await (await fetch(`http://${upHost}:${upPort}/json/list`)).json();
      const page = (Array.isArray(list) ? list : []).find((t) => t.type === 'page') ?? list[0];
      if (!page?.id) throw new Error('no page target upstream');
      const c = new CdpClient(`ws://${upHost}:${upPort}/devtools/page/${page.id}`, { connectTimeoutMs: 10_000 });
      await c.connect();
      pageChannel = c;
      console.log(`mitm: page channel -> /devtools/page/${page.id}`);
      return c;
    })();
    try {
      return await pageChannelConnecting;
    } catch (e) {
      pageChannel = null;
      throw e;
    } finally {
      pageChannelConnecting = null;
    }
  };
  const pageSend = async (method, params) => {
    try {
      const c = await getPageChannel();
      const r = await c.send(method, params, { timeoutMs: 10_000, raw: true });
      if (r.status === 'result') return { ok: true, result: r.result };
      return { ok: false, error: r.error ?? { code: -32000, message: `mitm: page channel ${r.status}` } };
    } catch (e) {
      return { ok: false, error: { code: -32000, message: e.message } };
    }
  };

  const server = createServer(async (req, res) => {
    try {
      const upstream = await fetch(`http://${upHost}:${upPort}${req.url}`);
      const text = await upstream.text();
      let body = text;
      try {
        const json = JSON.parse(text);
        const rewrite = (o) => {
          if (o && typeof o === 'object') {
            if (typeof o.webSocketDebuggerUrl === 'string') o.webSocketDebuggerUrl = o.webSocketDebuggerUrl.replace(/^ws:\/\/[^/]+/, `ws://${args.host}:${args.listen}`);
            for (const v of Object.values(o)) rewrite(v);
          }
        };
        rewrite(json);
        body = JSON.stringify(json);
      } catch {}
      res.writeHead(upstream.status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
      res.end(body);
    } catch (e) {
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end(`mitm: upstream error: ${e.message}`);
    }
  });

  server.on('upgrade', (req, socket) => {
    const key = req.headers['sec-websocket-key'];
    const accept = createHash('sha1').update(key + GUID).digest('base64');
    socket.write(
      'HTTP/1.1 101 Switching Protocols\r\n' +
        'Upgrade: websocket\r\n' +
        'Connection: Upgrade\r\n' +
        `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    socket.setNoDelay(true);

    const upUrl = `ws://${upHost}:${upPort}${req.url}`;
    const upstreamWs = new WebSocket(upUrl);
    console.log(`mitm: ${req.url} -> ${upUrl}`);

    const idToMethod = new Map();
    const pendingUpstream = [];
    const subRequests = new Map(); // sub-request id -> {resolve}
    let subIdSeq = 900_000;
    let channel = null;
    const started = Date.now();

    const toClient = (obj, extra = {}) => {
      logLine({ dir: 's2c', at: Date.now() - started, ...extra, msg: obj });
      channel?.sendText(JSON.stringify(obj));
    };
    const sendUpstream = (obj) => {
      const text = JSON.stringify(obj);
      if (upstreamWs.readyState === 1) upstreamWs.send(text);
      else if (upstreamWs.readyState === 0) pendingUpstream.push(text);
      else throw new Error('mitm: upstream not connected');
    };

    // A "rewrite" turns a client command into one upstream command per item
    // (e.g. Storage.setCookies -> N x Network.setCookie) and answers the client
    // only once every sub-request settled. Qt implements the Network.* cookie
    // commands (on a page session) but not the Storage.* / browser-context ones.
    const handleRewrite = async (msg) => {
      const rw = rewrites[msg.method];
      const items = rw.split ? msg.params?.[rw.split] ?? [] : [null];
      logLine({ dir: 'c2s', at: Date.now() - started, rewritten: rw.as, msg });
      const omit = (params, keys) => {
        const out = { ...(params ?? {}) };
        for (const k of keys) delete out[k];
        return out;
      };
      const results = await Promise.all(
        items.map((item) => {
          const params = rw.split ? item : rw.params !== undefined ? subst(rw.params) : omit(msg.params, ['browserContextId']);
          if (rw.on === 'page') return pageSend(rw.as, params);
          return new Promise((resolve) => {
            const subId = ++subIdSeq;
            subRequests.set(subId, { resolve });
            logLine({ dir: 'c2s', at: Date.now() - started, rewritten: rw.as, msg: { id: subId, method: rw.as, params } });
            try {
              sendUpstream({ id: subId, method: rw.as, ...(params !== undefined ? { params } : {}) });
            } catch (e) {
              subRequests.delete(subId);
              resolve({ ok: false, error: { code: -32000, message: e.message } });
            }
          });
        }),
      );
      const failure = results.find((r) => r && !r.ok);
      if (failure) {
        toClient({ id: msg.id, error: failure.error ?? { code: -32000, message: 'mitm: rewrite failed' } }, { rewritten: true });
        return;
      }
      toClient({ id: msg.id, result: rw.split ? rw.result ?? {} : results[0]?.result ?? {} }, { rewritten: true });
    };

    channel = new ServerSocketChannel(
      socket,
      (text) => {
        let msg;
        try {
          msg = JSON.parse(text);
        } catch {
          logLine({ dir: 'c2s', at: Date.now() - started, raw: text.slice(0, 400) });
          return;
        }
        if (msg.id !== undefined && requestStubs[msg.method]) {
          const stub = subst(requestStubs[msg.method]);
          logLine({ dir: 'c2s', at: Date.now() - started, stubbed: true, msg });
          toClient({ id: msg.id, ...(stub.error ? { error: stub.error } : { result: stub.result ?? {} }) }, { stubbed: true });
          for (const ev of injectEvents[msg.method] ?? []) toClient(subst(ev), { injected: true });
          return;
        }
        if (msg.id !== undefined && rewrites[msg.method]) {
          handleRewrite(msg).catch((e) => {
            toClient({ id: msg.id, error: { code: -32000, message: `mitm: rewrite failed: ${e.message}` } }, { rewritten: true });
          });
          return;
        }
        logLine({ dir: 'c2s', at: Date.now() - started, stubbed: false, msg });
        if (msg.id !== undefined) idToMethod.set(msg.id, msg.method);
        if (upstreamWs.readyState === 1) upstreamWs.send(text);
        else if (upstreamWs.readyState === 0) pendingUpstream.push(text);
        else if (msg.id !== undefined) toClient({ id: msg.id, error: { code: -32000, message: 'mitm: upstream not connected' } });
      },
      () => {
        try {
          upstreamWs.close();
        } catch {}
      },
    );

    upstreamWs.addEventListener('open', () => {
      for (const t of pendingUpstream) upstreamWs.send(t);
      pendingUpstream.length = 0;
    });
    upstreamWs.addEventListener('message', (ev) => {
      const text = String(ev.data);
      let msg;
      try {
        msg = JSON.parse(text);
      } catch {
        logLine({ dir: 's2c', at: Date.now() - started, raw: text.slice(0, 400) });
        return;
      }
      const sub = msg.id !== undefined ? subRequests.get(msg.id) : undefined;
      if (sub !== undefined) {
        subRequests.delete(msg.id);
        logLine({ dir: 's2c', at: Date.now() - started, rewritten: true, msg });
        sub.resolve(msg.error ? { ok: false, error: msg.error } : { ok: true, result: msg.result });
        return;
      }
      toClient(msg);
      if (msg.id !== undefined && idToMethod.has(msg.id)) {
        const method = idToMethod.get(msg.id);
        idToMethod.delete(msg.id);
        for (const ev2 of injectEvents[method] ?? []) toClient(subst(ev2), { injected: true });
      }
    });
    upstreamWs.addEventListener('close', () => channel && channel.socket.end(encodeClose(1000)));
    upstreamWs.addEventListener('error', () => {
      try {
        channel.socket.destroy();
      } catch {}
    });
  });

  server.listen(args.listen, args.host, () => {
    console.log(`mitm: listening on http://${args.host}:${args.listen} -> ${args.upstream}`);
    console.log(`mitm: log ${args.log ?? '(none)'} stubs ${args.stubs ?? '(none)'}`);
  });
}

main();
