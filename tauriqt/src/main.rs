// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! TauriQt — a Tauri 2 app that runs on the Qt runtime (Qt windowing +
//! QtWebEngine) on Linux and falls back to the stock wry runtime elsewhere
//! (or on Linux when built with `--features wry`).

/// Runtime selection: Qt on Linux, wry everywhere else (and with `--features wry`).
#[cfg(all(target_os = "linux", not(feature = "wry")))]
pub type AppRuntime = tauri_runtime_qt::QtRuntime;
#[cfg(any(not(target_os = "linux"), feature = "wry"))]
pub type AppRuntime = tauri::Wry;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! — from {}", std::env::consts::OS)
}

/// Renders one probed window method as `label=value`, or `label=ERR(message)`
/// when the runtime cannot answer truthfully on this platform. Never panics and
/// never substitutes a fabricated value — a failing probe is reported as such.
fn probe<T>(label: &str, result: tauri::Result<T>, render: impl FnOnce(T) -> String) -> String {
    match result {
        Ok(value) => format!("{label}={}", render(value)),
        Err(err) => format!("{label}=ERR({err})"),
    }
}

/// Exercises the `WindowDispatch` read path: geometry, scale factor, title and
/// every `is_*` flag. Each method is probed independently so one unsupported
/// call does not hide the others.
#[tauri::command]
fn platform() -> String {
    runtime_platform_name()
}

/// The Qt runtime knows its platform plugin; the wry fallback does not need to.
#[cfg(all(target_os = "linux", not(feature = "wry")))]
fn runtime_platform_name() -> String {
    tauri_runtime_qt::platform_name()
}

#[cfg(any(not(target_os = "linux"), feature = "wry"))]
fn runtime_platform_name() -> String {
    "wry".to_owned()
}

#[tauri::command]
fn win_info(window: tauri::WebviewWindow<AppRuntime>) -> String {
    let fields = [
        probe("inner_size", window.inner_size(), |s| {
            format!("{}x{}", s.width, s.height)
        }),
        probe("outer_size", window.outer_size(), |s| {
            format!("{}x{}", s.width, s.height)
        }),
        probe("inner_position", window.inner_position(), |p| {
            format!("{},{}", p.x, p.y)
        }),
        probe("outer_position", window.outer_position(), |p| {
            format!("{},{}", p.x, p.y)
        }),
        probe("scale_factor", window.scale_factor(), |s| format!("{s:.4}")),
        probe("title", window.title(), |t| t),
        probe("is_fullscreen", window.is_fullscreen(), |v| v.to_string()),
        probe("is_minimized", window.is_minimized(), |v| v.to_string()),
        probe("is_maximized", window.is_maximized(), |v| v.to_string()),
        probe("is_focused", window.is_focused(), |v| v.to_string()),
        probe("is_decorated", window.is_decorated(), |v| v.to_string()),
        probe("is_resizable", window.is_resizable(), |v| v.to_string()),
        probe("is_visible", window.is_visible(), |v| v.to_string()),
        probe("is_always_on_top", window.is_always_on_top(), |v| {
            v.to_string()
        }),
        probe("is_maximizable", window.is_maximizable(), |v| v.to_string()),
        probe("is_minimizable", window.is_minimizable(), |v| v.to_string()),
        probe("is_closable", window.is_closable(), |v| v.to_string()),
    ];
    fields.join("; ")
}

/// Renders a monitor as `name pos size work scale`.
fn fmt_monitor(index: usize, monitor: &tauri::Monitor) -> String {
    format!(
        "#{index} name={:?} pos={},{} size={}x{} work={}x{} scale={:.4}",
        monitor.name(),
        monitor.position().x,
        monitor.position().y,
        monitor.size().width,
        monitor.size().height,
        monitor.work_area().size.width,
        monitor.work_area().size.height,
        monitor.scale_factor(),
    )
}

/// Exercises the monitor path: available monitors, primary monitor and the
/// monitor the window currently sits on.
#[tauri::command]
fn monitors(window: tauri::WebviewWindow<AppRuntime>) -> String {
    let available = match window.available_monitors() {
        Ok(list) => {
            let mut rendered = format!("available_monitors={}", list.len());
            for (index, monitor) in list.iter().enumerate() {
                rendered.push_str(" | ");
                rendered.push_str(&fmt_monitor(index, monitor));
            }
            rendered
        }
        Err(err) => format!("available_monitors=ERR({err})"),
    };
    let primary = probe("primary_monitor", window.primary_monitor(), |m| {
        m.map(|monitor| fmt_monitor(0, &monitor))
            .unwrap_or_else(|| "none".to_string())
    });
    let current = probe("current_monitor", window.current_monitor(), |m| {
        m.map(|monitor| fmt_monitor(0, &monitor))
            .unwrap_or_else(|| "none".to_string())
    });
    format!("{available}; {primary}; {current}")
}

/// Sets the window fullscreen state. Surfaces the runtime error as an invoke
/// rejection instead of pretending it succeeded.
#[tauri::command]
fn set_fullscreen(
    window: tauri::WebviewWindow<AppRuntime>,
    fullscreen: bool,
) -> Result<String, String> {
    window
        .set_fullscreen(fullscreen)
        .map(|()| format!("set_fullscreen({fullscreen})=OK"))
        .map_err(|err| format!("set_fullscreen({fullscreen}) failed: {err}"))
}

/// Sets the window title. Surfaces the runtime error as an invoke rejection.
#[tauri::command]
fn set_title(window: tauri::WebviewWindow<AppRuntime>, title: String) -> Result<String, String> {
    window
        .set_title(&title)
        .map(|()| format!("set_title({title:?})=OK"))
        .map_err(|err| format!("set_title({title:?}) failed: {err}"))
}

/// Logs a line from the frontend to the terminal so demo results are visible
/// outside the web view.
#[tauri::command]
fn report(text: String) -> String {
    println!("[tauriqt-demo] {text}");
    "reported".to_string()
}

// ---------------------------------------------------------------------------
// Cookie demo (the runtime mirrors the profile's cookie store)
// ---------------------------------------------------------------------------
//
// `tauri://` is a custom scheme and Chromium only lets a document read or write
// cookies on a *cookieable* scheme (`http`, `https`, `ws`, `wss`) — the same
// reason tauri's own `Webview::cookies()` documentation excludes `tauri://`
// URLs. `document.cookie` is therefore inert on this page. To exercise real
// JS-side cookie access (and to prove that a JS-set cookie reaches the
// runtime's mirror) the page embeds a sandbox document served from
// `http://127.0.0.1` on an ephemeral port: a different origin, but the same
// webview, the same profile and therefore the same cookie store that
// `cookies()`, `cookies_for_url()`, `set_cookie()` and `delete_cookie()` see.

/// The sandbox document: a plain http page whose only job is to run
/// `document.cookie` operations on request and report the result — the cookie
/// it read back — to the parent page.
const COOKIE_SANDBOX_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>cookie sandbox</title>
    <style>
      body { font-family: ui-monospace, monospace; font-size: 0.8rem; margin: 0.5rem; }
      code { word-break: break-all; }
    </style>
  </head>
  <body>
    <div>cookie sandbox on <code id="origin"></code></div>
    <div>document.cookie: <code id="value"></code></div>
    <script>
      // Runs on http://127.0.0.1:<port>, an origin Chromium allows cookies on
      // (the parent page's tauri:// scheme does not). Only the parent's
      // postMessage protocol reaches this document.
      const origin = document.getElementById("origin");
      const value = document.getElementById("value");
      origin.textContent = location.origin;

      function read() {
        try {
          return document.cookie;
        } catch (err) {
          return "ERR(" + err + ")";
        }
      }

      function show() {
        value.textContent = JSON.stringify(read());
        return read();
      }

      window.addEventListener("message", (event) => {
        const { id, op, cookie } = event.data || {};
        let result;
        switch (op) {
          case "set": // a document.cookie write, e.g. "name=value; path=/"
            document.cookie = cookie;
            result = show();
            break;
          case "read":
            result = show();
            break;
          default:
            result = "ERR(unknown op " + op + ")";
        }
        parent.postMessage({ id, op, result }, "*");
      });

      parent.postMessage({ op: "ready", result: show() }, "*");
    </script>
  </body>
</html>
"#;

/// Port of the sandbox's loopback server. The server starts with the first use
/// of [`cookie_sandbox_url`] and lives as long as the process.
static COOKIE_SANDBOX_PORT: std::sync::LazyLock<Option<u16>> =
    std::sync::LazyLock::new(start_cookie_sandbox);

/// Serves [`COOKIE_SANDBOX_HTML`] on an ephemeral 127.0.0.1 port and returns it.
/// Minimal on purpose: one request per connection, loopback only, so it never
/// outlives the demo process.
fn start_cookie_sandbox() -> Option<u16> {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    std::thread::Builder::new()
        .name("tauriqt-cookie-sandbox".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 1024];
                let read = stream.read(&mut request).unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..read]);
                let wants_page =
                    head.starts_with("GET / ") || head.starts_with("GET /index.html ");
                let (status, body) = if wants_page {
                    ("200 OK", COOKIE_SANDBOX_HTML)
                } else {
                    ("404 Not Found", "")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        })
        .ok()?;
    Some(port)
}

/// URL of the cookie sandbox document (starts its server on first call).
#[tauri::command]
fn cookie_sandbox_url() -> Option<String> {
    (*COOKIE_SANDBOX_PORT).map(|port| format!("http://127.0.0.1:{port}/"))
}

/// Renders one cookie for the terminal report.
fn fmt_cookie(cookie: &tauri::webview::Cookie<'_>) -> String {
    let mut out = format!("{}={}", cookie.name(), cookie.value());
    if let Some(domain) = cookie.domain() {
        out.push_str(&format!(" domain={domain}"));
    }
    if let Some(path) = cookie.path() {
        out.push_str(&format!(" path={path}"));
    }
    if cookie.secure() == Some(true) {
        out.push_str(" secure");
    }
    if cookie.http_only() == Some(true) {
        out.push_str(" http_only");
    }
    if let Some(same_site) = cookie.same_site() {
        out.push_str(&format!(" same_site={same_site:?}"));
    }
    match cookie.expires_datetime() {
        Some(expires) => out.push_str(&format!(
            " expires_ms={}",
            expires.unix_timestamp() * 1000 + i64::from(expires.millisecond())
        )),
        None => out.push_str(" session"),
    }
    out
}

/// Dumps the runtime's cookie mirror (`WebviewDispatch::cookies`), or — when
/// `url` is given — the cookies a request to that URL would carry
/// (`WebviewDispatch::cookies_for_url`).
#[tauri::command]
fn cookies_dump(
    window: tauri::WebviewWindow<AppRuntime>,
    url: Option<String>,
) -> Result<String, String> {
    let cookies = match url {
        Some(url) => {
            let parsed =
                tauri::Url::parse(&url).map_err(|err| format!("invalid url {url:?}: {err}"))?;
            window.cookies_for_url(parsed)
        }
        None => window.cookies(),
    }
    .map_err(|err| format!("cookies_dump failed: {err}"))?;

    if cookies.is_empty() {
        return Ok("(no cookies)".to_string());
    }
    Ok(cookies
        .iter()
        .map(fmt_cookie)
        .collect::<Vec<_>>()
        .join(" | "))
}

/// Builds a cookie for the runtime's `set_cookie` from `Set-Cookie`-style
/// attributes. `same_site` must be `none`, `lax` or `strict` when given — the
/// cookie crate's parser silently ignores unknown SameSite values, so they are
/// rejected here instead of turning into an attribute-less cookie.
fn build_cookie(
    name: &str,
    value: &str,
    domain: Option<&str>,
    path: Option<&str>,
    secure: bool,
    same_site: Option<&str>,
) -> Result<tauri::webview::Cookie<'static>, String> {
    let mut text = format!("{name}={value}; path={}", path.unwrap_or("/"));
    if let Some(domain) = domain {
        text.push_str(&format!("; domain={domain}"));
    }
    if secure {
        text.push_str("; Secure");
    }
    if let Some(same_site) = same_site {
        match same_site.to_ascii_lowercase().as_str() {
            "none" | "lax" | "strict" => text.push_str(&format!("; SameSite={same_site}")),
            other => {
                return Err(format!("same_site {other:?} must be none, lax or strict"));
            }
        }
    }
    match tauri::webview::Cookie::parse(text.clone()) {
        Ok(cookie) => Ok(cookie),
        Err(err) => Err(format!("invalid cookie attributes {text:?}: {err}")),
    }
}

/// Sets a cookie through the runtime (`WebviewDispatch::set_cookie`). Without a
/// `domain` the cookie becomes host-only for the webview's current page host.
#[tauri::command]
fn cookie_set(
    window: tauri::WebviewWindow<AppRuntime>,
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: Option<bool>,
    same_site: Option<String>,
) -> Result<String, String> {
    let cookie = build_cookie(
        &name,
        &value,
        domain.as_deref(),
        path.as_deref(),
        secure.unwrap_or(false),
        same_site.as_deref(),
    )?;
    window
        .set_cookie(cookie)
        .map_err(|err| format!("cookie_set({name}) failed: {err}"))?;
    Ok(format!("cookie_set({name}) OK"))
}

/// Deletes cookies through the runtime (`WebviewDispatch::delete_cookie`).
#[tauri::command]
fn cookie_delete(
    window: tauri::WebviewWindow<AppRuntime>,
    name: String,
    domain: Option<String>,
    path: Option<String>,
) -> Result<String, String> {
    let mut builder = tauri::webview::Cookie::build((name.clone(), ""))
        .path(path.unwrap_or_else(|| "/".to_string()));
    if let Some(domain) = domain {
        builder = builder.domain(domain);
    }
    window
        .delete_cookie(builder.build())
        .map_err(|err| format!("cookie_delete({name}) failed: {err}"))?;
    Ok(format!("cookie_delete({name}) OK"))
}

/// `WebviewDispatch::clear_all_browsing_data`: every cookie of the profile plus
/// the HTTP cache (see the runtime's `ops::cookies::clear_all_browsing_data`
/// for what Qt cannot clear).
#[tauri::command]
fn cookies_clear(window: tauri::WebviewWindow<AppRuntime>) -> Result<String, String> {
    window
        .clear_all_browsing_data()
        .map_err(|err| format!("clear_all_browsing_data failed: {err}"))?;
    Ok("clear_all_browsing_data OK".to_string())
}

fn main() {
    let mut builder = tauri::Builder::<AppRuntime>::new().invoke_handler(tauri::generate_handler![greet,
        win_info,
        monitors,
        set_fullscreen,
        set_title,
        report,
        cookie_sandbox_url,
        cookies_dump,
        cookie_set,
        cookie_delete,
        cookies_clear, platform]);

    // The Qt runtime carries invoke payloads in a header (QtWebEngine never
    // exposes custom-protocol request bodies), so it ships its own invoke system.
    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    {
        builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running the tauri application");
}
