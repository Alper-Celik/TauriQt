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
        probe("scale_factor", window.scale_factor(), |s| {
            format!("{s:.4}")
        }),
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
fn set_fullscreen(window: tauri::WebviewWindow<AppRuntime>, fullscreen: bool) -> Result<String, String> {
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

fn main() {
    let mut builder = tauri::Builder::<AppRuntime>::new().invoke_handler(tauri::generate_handler![
        greet,
        win_info,
        monitors,
        set_fullscreen,
        set_title,
        report
    ]);

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
