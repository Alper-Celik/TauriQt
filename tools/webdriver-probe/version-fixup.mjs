#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Minimal HTTP fix-up in front of QtWebEngine's DevTools endpoint.
//
// Why: QtWebEngine answers `GET /json/version` with
//   "Browser": "<QCoreApplication::applicationName()>/<applicationVersion>"
// (TauriQt sets the Qt app name to "tauri-runtime-qt" and no version, so the
// field reads "tauri-runtime-qt/"). ChromeDriver parses that field with
// BrowserInfo::ParseBrowserString and aborts session creation with
//   "unrecognized Chrome version: tauri-runtime-qt/"
// before sending a single CDP command. This proxy rewrites just that JSON
// field to a Chrome-shaped version string and forwards everything else —
// including WebSocket upgrades — verbatim to the real DevTools port.
//
// Usage (standalone):
//   node version-fixup.mjs --port=9224 --target=127.0.0.1:9223 \
//        --browser-version="Chrome/140.0.0.0"

import { createServer } from "node:http";
import { connect } from "node:net";

export function startVersionFixup({ target, port, browserVersion }) {
  const [targetHost, targetPort] = target.split(":");
  const requests = [];

  const server = createServer(async (req, res) => {
    const url = `http://${target}${req.url}`;
    requests.push({ method: req.method, url: req.url });
    try {
      const upstream = await fetch(url);
      const text = await upstream.text();
      let body = text;
      if (req.url === "/json/version" || req.url === "/json" || req.url === "/json/version/") {
        try {
          const parsed = JSON.parse(text);
          parsed.Browser = browserVersion;
          body = JSON.stringify(parsed, null, 3);
        } catch {
          /* not JSON: pass through */
        }
      }
      res.writeHead(upstream.status, {
        "content-type": upstream.headers.get("content-type") || "application/json; charset=UTF-8",
        "content-length": Buffer.byteLength(body),
      });
      res.end(body);
    } catch (err) {
      res.writeHead(502, { "content-type": "text/plain" });
      res.end(`fixup proxy: ${err}\n`);
    }
  });

  // WebSocket upgrades (if ChromeDriver ever dials the proxy instead of the
  // advertised ws:// URL) are piped to the real endpoint untouched.
  server.on("upgrade", (req, socket, head) => {
    const upstream = connect(Number(targetPort), targetHost, () => {
      let raw = `${req.method} ${req.url} HTTP/1.1\r\n`;
      for (let i = 0; i < req.rawHeaders.length; i += 2) {
        raw += `${req.rawHeaders[i]}: ${req.rawHeaders[i + 1]}\r\n`;
      }
      raw += "\r\n";
      upstream.write(raw);
      if (head?.length) upstream.write(head);
      socket.pipe(upstream).pipe(socket);
    });
    upstream.on("error", () => socket.destroy());
    socket.on("error", () => upstream.destroy());
  });

  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () =>
      resolve({
        server,
        port: server.address().port,
        requests,
        close: () => new Promise((done) => server.close(done)),
      }),
    );
  });
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const args = new Map(
    process.argv.slice(2).map((arg) => {
      const [key, value = ""] = arg.replace(/^--/, "").split("=");
      return [key, value];
    }),
  );
  const proxy = await startVersionFixup({
    target: args.get("target") || "127.0.0.1:9223",
    port: Number(args.get("port") || 9224),
    browserVersion: args.get("browser-version") || "Chrome/140.0.0.0",
  });
  console.log(`version-fixup proxy on 127.0.0.1:${proxy.port} -> ${args.get("target") || "127.0.0.1:9223"} (Browser=${args.get("browser-version") || "Chrome/140.0.0.0"})`);
}
