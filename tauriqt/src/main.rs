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

fn main() {
    let mut builder = tauri::Builder::<AppRuntime>::new()
        .invoke_handler(tauri::generate_handler![greet]);

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
