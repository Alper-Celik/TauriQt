#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT
//
// Measured CDP capability probe for QtWebEngine (TauriQt runtime).
//
//   node tools/cdp-probe/probe.mjs [--host 127.0.0.1] [--port 9222]
//        [--out tools/cdp-probe/last-run.json] [--timeout-ms 5000]
//        [--target-id <id|url-substring>] [--quiet]
//
// 1. GET /json/version and /json/list and print them.
// 2. Open a WebSocket to the chosen page target.
// 3. Send the fixed command battery one command at a time; record, per command,
//    the full result (structurally truncated), or the verbatim
//    `error {code, message}`, or a timeout (with the timeout used).
// 4. Passively record every protocol event that arrives (console/exception
//    visibility is the interesting part), with per-method counts.
// 5. Write the whole transcript to --out and print a compact table.
//
// Exit codes: 0 = run completed (individual command errors are data, not
// failures), 2 = no CDP endpoint / websocket connect failed.
//
// No npm dependencies: Node's global WebSocket (Node >= 22) only.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { CdpClient, httpJson, sleep, summarize, waitForDevtools } from './cdp-lib.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));

function parseArgs(argv) {
  const args = {
    host: '127.0.0.1',
    port: 9222,
    out: resolve(HERE, 'last-run.json'),
    timeoutMs: 5000,
    targetId: null,
    quiet: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    switch (a) {
      case '--host': args.host = next(); break;
      case '--port': args.port = Number(next()); break;
      case '--out': args.out = resolve(next()); break;
      case '--timeout-ms': args.timeoutMs = Number(next()); break;
      case '--target-id': args.targetId = next(); break;
      case '--quiet': args.quiet = true; break;
      case '-h':
      case '--help':
        console.log('usage: probe.mjs [--host H] [--port N] [--out FILE] [--timeout-ms N] [--target-id ID] [--quiet]');
        process.exit(0);
      default:
        throw new Error(`unknown argument: ${a}`);
    }
  }
  return args;
}

/**
 * The measured battery. `params` may be a function of the run context (used for
 * `DOM.querySelector`, which needs the nodeId returned by `DOM.getDocument`).
 * `skip` marks a step as unrunnable without its prerequisite.
 */
function battery(ctx) {
  const pageUrl = ctx.target.url;
  return [
    // ---- Browser domain (Playwright wires window bounds/downloads here) ----
    { phase: 'browser', method: 'Browser.getVersion' },
    { phase: 'browser', method: 'Browser.getBrowserCommandLine' },
    { phase: 'browser', method: 'Browser.getWindowForTarget', params: { targetId: ctx.target.id } },
    { phase: 'browser', method: 'Browser.getWindowBounds', params: { windowId: 1 } },
    {
      phase: 'browser',
      method: 'Browser.setDownloadBehavior',
      // Playwright's exact shape (new API): behaviour + path + event stream.
      params: { behavior: 'allow', downloadPath: '/tmp/cdp-probe-downloads', eventsEnabled: true },
    },

    // ---- Target domain (the attach path) ----
    { phase: 'targets', method: 'Target.getTargets' },
    { phase: 'targets', method: 'Target.getBrowserContexts' },
    { phase: 'targets', method: 'Target.setDiscoverTargets', params: { discover: true } },
    { phase: 'targets', method: 'Target.getTargetInfo', params: { targetId: ctx.target.id } },
    {
      phase: 'targets',
      method: 'Target.setAutoAttach',
      params: { autoAttach: true, waitForDebuggerOnStart: false, flatten: true },
    },
    {
      phase: 'targets',
      method: 'Target.setAutoAttach',
      note: 'Playwright shape (waitForDebuggerOnStart + filter)',
      params: {
        autoAttach: true,
        waitForDebuggerOnStart: true,
        flatten: true,
        filter: [{ type: 'browser', exclude: true }, { type: 'tab', exclude: true }, { type: 'page', exclude: false }],
      },
    },
    {
      phase: 'targets',
      method: 'Target.attachToTarget',
      params: { targetId: ctx.target.id, flatten: true },
      after: (c, rec) => {
        if (rec.status === 'result') c.sessionId = rec.result?.sessionId;
      },
    },
    {
      phase: 'targets',
      method: 'Runtime.evaluate',
      note: 'flat session: command with sessionId',
      sessionId: (c) => c.sessionId,
      params: { expression: '"session-ok: " + document.title', returnByValue: true },
      skip: (c) => !c.sessionId,
      skipReason: 'no sessionId from Target.attachToTarget',
    },
    {
      phase: 'targets',
      method: 'Page.getFrameTree',
      note: 'flat session: page command with sessionId',
      sessionId: (c) => c.sessionId,
      skip: (c) => !c.sessionId,
      skipReason: 'no sessionId from Target.attachToTarget',
    },
    {
      phase: 'targets',
      method: 'Target.detachFromTarget',
      params: (c) => ({ sessionId: c.sessionId }),
      skip: (c) => !c.sessionId,
      skipReason: 'no sessionId from Target.attachToTarget',
    },

    // ---- Enables (events start flowing from here on) ----
    { phase: 'enables', method: 'Page.enable' },
    { phase: 'enables', method: 'Runtime.enable' },
    { phase: 'enables', method: 'Log.enable' },
    { pause: 750, note: 'settle: passive event window after Page/Runtime/Log.enable' },

    // ---- Page / DOM / CSS ----
    { phase: 'dom', method: 'Page.getFrameTree' },
    { phase: 'dom', method: 'Page.setLifecycleEventsEnabled', params: { enabled: true } },
    {
      phase: 'dom',
      method: 'Page.addScriptToEvaluateOnNewDocument',
      note: 'Playwright installs init scripts this way',
      params: { source: 'window.__cdpProbeInit = true;' },
      after: (c, rec) => {
        if (rec.status === 'result') c.initScriptId = rec.result?.identifier;
      },
    },
    {
      phase: 'dom',
      method: 'Page.removeScriptToEvaluateOnNewDocument',
      params: (c) => ({ identifier: c.initScriptId }),
      skip: (c) => !c.initScriptId,
      skipReason: 'no identifier from Page.addScriptToEvaluateOnNewDocument',
    },
    {
      phase: 'dom',
      method: 'Page.createIsolatedWorld',
      params: (c) => ({ frameId: c.target.id, worldName: '__cdp_probe_world', grantUniveralAccess: false }),
    },
    {
      phase: 'dom',
      method: 'DOM.getDocument',
      params: { depth: 1 },
      after: (c, rec) => {
        if (rec.status === 'result') c.rootNodeId = rec.result?.root?.nodeId;
      },
    },
    {
      phase: 'dom',
      method: 'DOM.querySelector',
      params: (c) => ({ nodeId: c.rootNodeId, selector: '#summary' }),
      skip: (c) => c.rootNodeId == null,
      skipReason: 'no root nodeId (DOM.getDocument failed)',
    },
    { phase: 'dom', method: 'CSS.enable' },

    // ---- Runtime: evaluation, console/exception visibility, Tauri invoke ----
    { phase: 'runtime', method: 'Runtime.evaluate', params: { expression: '1+1', returnByValue: true } },
    { phase: 'runtime', method: 'Runtime.evaluate', params: { expression: 'document.title', returnByValue: true } },
    {
      phase: 'runtime',
      method: 'Runtime.evaluate',
      note: 'console visibility probe',
      params: {
        expression: '(() => { console.log("cdp-probe: console.log"); console.warn("cdp-probe: console.warn"); return "logged"; })()',
        returnByValue: true,
      },
    },
    {
      phase: 'runtime',
      method: 'Runtime.evaluate',
      note: 'exception visibility probe',
      params: {
        expression: '(() => { setTimeout(() => { throw new Error("cdp-probe-uncaught"); }, 0); return "scheduled"; })()',
        returnByValue: true,
      },
    },
    {
      phase: 'runtime',
      method: 'Runtime.evaluate',
      note: 'end-to-end: Tauri invoke("greet") through the page',
      params: {
        expression: 'window.__TAURI_INTERNALS__.invoke("greet",{name:"cdp"})',
        awaitPromise: true,
        returnByValue: true,
      },
    },
    {
      phase: 'runtime',
      method: 'Runtime.evaluate',
      note: 'end-to-end: Tauri invoke("report") reaches the Rust side',
      params: {
        expression: 'window.__TAURI_INTERNALS__.invoke("report",{text:"cdp-probe: evaluate reached Rust"})',
        awaitPromise: true,
        returnByValue: true,
      },
    },
    { pause: 750, note: 'settle: console/exception events after the probes' },

    // ---- Network cookies ----
    { phase: 'network', method: 'Network.enable' },
    { phase: 'network', method: 'Network.getAllCookies' },
    { phase: 'network', method: 'Network.getCookies', params: { urls: [pageUrl] } },
    { phase: 'network', method: 'Network.setCookie', params: { name: 'cdp_probe', value: '1', url: pageUrl, path: '/' } },
    { phase: 'network', method: 'Network.setCookie', note: 'http:// target', params: { name: 'cdp_probe_http', value: '1', url: 'http://localhost/', path: '/' } },
    { phase: 'network', method: 'Network.getAllCookies', note: 'after setCookie' },
    { phase: 'network', method: 'Storage.getCookies' },
    { phase: 'network', method: 'Network.deleteCookies', params: { name: 'cdp_probe', url: pageUrl } },
    { phase: 'network', method: 'Network.deleteCookies', note: 'http:// target', params: { name: 'cdp_probe_http', url: 'http://localhost/' } },
    { phase: 'network', method: 'Network.clearBrowserCookies' },

    // ---- Input: arm observers, dispatch, then read the effect back ----
    {
      phase: 'input',
      method: 'Runtime.evaluate',
      note: 'arm click/key/input observers + contenteditable target',
      params: {
        expression: `(() => {
  window.__cdpProbe = { clicks: [], keys: [], inputs: [] };
  document.addEventListener('click', (e) => window.__cdpProbe.clicks.push({ x: e.clientX, y: e.clientY, isTrusted: e.isTrusted }), true);
  document.addEventListener('keydown', (e) => window.__cdpProbe.keys.push({ key: e.key, isTrusted: e.isTrusted }), true);
  document.addEventListener('input', (e) => window.__cdpProbe.inputs.push({ inputType: e.inputType, data: e.data, isTrusted: e.isTrusted }), true);
  const box = document.createElement('div');
  box.id = 'cdp-probe-input';
  box.contentEditable = 'true';
  box.style.cssText = 'position:fixed;left:0;top:0;width:300px;height:120px;border:1px solid #999;background:#fff';
  document.body.appendChild(box);
  box.focus();
  return document.activeElement.id;
})()`,
        returnByValue: true,
      },
    },
    { phase: 'input', method: 'Input.dispatchMouseEvent', params: { type: 'mousePressed', x: 40, y: 60, button: 'left', clickCount: 1 } },
    { phase: 'input', method: 'Input.dispatchMouseEvent', params: { type: 'mouseReleased', x: 40, y: 60, button: 'left', clickCount: 1 } },
    { phase: 'input', method: 'Input.insertText', params: { text: 'cdp-probe-text' } },
    { phase: 'input', method: 'Input.dispatchKeyEvent', params: { type: 'keyDown', key: 'a', code: 'KeyA', windowsVirtualKeyCode: 65, nativeVirtualKeyCode: 65, text: 'a' } },
    { phase: 'input', method: 'Input.dispatchKeyEvent', params: { type: 'keyUp', key: 'a', code: 'KeyA', windowsVirtualKeyCode: 65, nativeVirtualKeyCode: 65 } },
    {
      phase: 'input',
      method: 'Runtime.evaluate',
      note: 'read back: observed events + resulting text',
      params: {
        expression: `(() => {
  const box = document.getElementById('cdp-probe-input');
  const out = {
    clicks: window.__cdpProbe.clicks,
    keys: window.__cdpProbe.keys,
    inputs: window.__cdpProbe.inputs,
    activeElement: document.activeElement ? document.activeElement.id : null,
    boxText: box ? box.textContent : null,
  };
  if (box) box.remove();
  return JSON.stringify(out);
})()`,
        returnByValue: true,
      },
    },

    // ---- Emulation ----
    { phase: 'emulation', method: 'Emulation.setDeviceMetricsOverride', params: { width: 800, height: 600, deviceScaleFactor: 1, mobile: false } },
    { phase: 'emulation', method: 'Emulation.clearDeviceMetricsOverride' },

    // ---- Artifacts (slow: longer timeout) ----
    { phase: 'artifacts', method: 'Page.captureScreenshot', params: { format: 'png' }, timeoutMs: 15_000 },
    { phase: 'artifacts', method: 'Page.printToPDF', params: { printBackground: true }, timeoutMs: 15_000 },

    // ---- Security / Fetch ----
    { phase: 'security', method: 'Security.enable' },
    { phase: 'security', method: 'Fetch.enable', params: {} },
    { phase: 'security', method: 'Fetch.disable', note: 'cleanup: never leave interception armed' },

    // ---- Page.navigate last: it may reload the target ----
    { phase: 'navigate', method: 'Page.navigate', params: { url: pageUrl } },
    { pause: 1500, note: 'settle: navigation events' },
    {
      phase: 'navigate',
      method: 'Runtime.evaluate',
      note: 'after reload',
      params: { expression: 'document.title', returnByValue: true },
    },
    { pause: 500, note: 'settle: final drain' },
  ];
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const startedAt = new Date().toISOString();

  const version = await waitForDevtools(args.host, args.port, 20_000).catch((e) => {
    console.error(`probe: ${e.message}`);
    process.exit(2);
  });
  const list = await httpJson(args.host, args.port, '/json/list');

  if (!args.quiet) {
    console.log(`== /json/version (HTTP ${version.status}) ==`);
    console.log(JSON.stringify(version.body, null, 2));
    console.log(`\n== /json/list (HTTP ${list.status}) ==`);
    console.log(JSON.stringify(list.body, null, 2));
  }

  const targets = Array.isArray(list.body) ? list.body : [];
  let target;
  if (args.targetId) {
    target = targets.find((t) => t.id === args.targetId || String(t.id).startsWith(args.targetId) || String(t.url).includes(args.targetId));
  } else {
    target = targets.find((t) => t.type === 'page' && t.url?.startsWith('tauri://'))
      ?? targets.find((t) => t.type === 'page')
      ?? targets.find((t) => t.webSocketDebuggerUrl);
  }
  if (!target?.webSocketDebuggerUrl) {
    console.error(`probe: no usable target with webSocketDebuggerUrl in /json/list (${targets.length} target(s))`);
    process.exit(2);
  }

  // Rewrite the ws URL onto the host/port we were asked to probe: Qt may
  // advertise `localhost` and the firejail/network path may prefer 127.0.0.1.
  const wsUrl = String(target.webSocketDebuggerUrl).replace(/^ws:\/\/[^/]+/, `ws://${args.host}:${args.port}`);
  console.log(`\n== probed target ==\ntype=${target.type} id=${target.id}\nurl=${target.url}\nws=${wsUrl}`);

  const client = new CdpClient(wsUrl, { connectTimeoutMs: 10_000 });
  try {
    await client.connect();
  } catch (e) {
    console.error(`probe: ${e.message}`);
    process.exit(2);
  }

  const ctx = { target, rootNodeId: undefined };
  const records = [];
  const steps = battery(ctx);
  let index = 0;

  for (const step of steps) {
    if (step.pause) {
      records.push({ index: ++index, phase: step.phase ?? 'pause', method: '<pause>', status: 'pause', durationMs: step.pause, note: step.note });
      await sleep(step.pause);
      continue;
    }
    index += 1;
    const timeoutMs = step.timeoutMs ?? args.timeoutMs;
    if (step.skip?.(ctx)) {
      records.push({ index, phase: step.phase, method: step.method, status: 'skipped', skipReason: step.skipReason, timeoutMs });
      continue;
    }
    const params = typeof step.params === 'function' ? step.params(ctx) : step.params;
    const sessionId = typeof step.sessionId === 'function' ? step.sessionId(ctx) : step.sessionId;
    const rec = await client.send(step.method, params, { timeoutMs, sessionId });
    const record = { index, phase: step.phase, note: step.note, ...rec };
    records.push(record);
    step.after?.(ctx, record);
    const label = record.status === 'result' ? 'ok     ' : record.status === 'error' ? 'ERROR  ' : record.status === 'timeout' ? 'TIMEOUT' : record.status === 'connection-closed' ? 'CLOSED ' : 'skip   ';
    console.log(`[${String(index).padStart(2, ' ')}] ${label} ${step.method}${record.sessionId ? ` (session ${record.sessionId})` : ''} — ${summarize(step.method, record)}`);
  }

  // Extra: query the target list again — does Page.navigate keep the target id?
  const listAfter = await httpJson(args.host, args.port, '/json/list').catch((e) => ({ path: '/json/list', status: 0, body: String(e), parseError: e.message }));

  try {
    client.ws.close();
  } catch {}
  await sleep(150);

  const eventSummary = {};
  for (const ev of client.events) eventSummary[ev.method] = (eventSummary[ev.method] ?? 0) + 1;

  const transcript = {
    probe: {
      tool: 'tools/cdp-probe/probe.mjs',
      schemaVersion: 1,
      startedAt,
      finishedAt: new Date().toISOString(),
      node: process.version,
      host: args.host,
      port: args.port,
      defaultTimeoutMs: args.timeoutMs,
      targetSelection: args.targetId ?? 'first page target (prefers tauri:// URL)',
      target: { id: target.id, type: target.type, url: target.url, title: target.title, wsUrl },
      wsClosed: client.closedInfo ?? null,
    },
    http: {
      version,
      list,
      listAfterNavigate: listAfter,
    },
    events: client.events,
    eventSummary,
    commands: records,
  };

  mkdirSync(dirname(args.out), { recursive: true });
  writeFileSync(args.out, `${JSON.stringify(transcript, null, 2)}\n`);
  console.log(`\n== wrote ${args.out} (${records.length} steps, ${client.events.length} events) ==`);
  console.log('\n== event summary (passive, after enables) ==');
  const methods = Object.keys(eventSummary).sort();
  if (methods.length === 0) console.log('(no protocol events arrived)');
  for (const m of methods) console.log(`  ${String(eventSummary[m]).padStart(3, ' ')}  ${m}`);
  console.log('\n== compact table ==');
  const rows = records.map((r) => ({
    n: String(r.index),
    method: r.method === '<pause>' ? `(pause ${r.durationMs} ms)` : r.method,
    status: r.status,
    detail: summarize(r.method, r),
  }));
  const w = (k) => Math.max(...rows.map((r) => r[k].length), k.length);
  const [wn, wm, ws] = [w('n'), w('method'), w('status')];
  console.log(`${'#'.padEnd(wn)}  ${'method'.padEnd(wm)}  ${'status'.padEnd(ws)}  detail`);
  for (const r of rows) console.log(`${r.n.padEnd(wn)}  ${r.method.padEnd(wm)}  ${r.status.padEnd(ws)}  ${r.detail}`);
}

main().catch((e) => {
  console.error(`probe: unexpected failure: ${e.stack || e}`);
  process.exit(2);
});
