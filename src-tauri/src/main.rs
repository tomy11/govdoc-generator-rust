// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use tauri::{Manager, RunEvent};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

/// Holds the spawned `govdoc-api` sidecar so it can be killed on exit.
#[derive(Default)]
struct ApiSidecar(Mutex<Option<CommandChild>>);

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(ApiSidecar::default())
        .setup(|app| {
            let addr =
                std::env::var("GOVDOC_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());

            // Resolve and launch the bundled API binary (binaries/govdoc-api-<triple>).
            let command = app.shell().sidecar("govdoc-api")?.env("GOVDOC_API_ADDR", addr);
            let (mut rx, child) = command.spawn()?;
            app.state::<ApiSidecar>()
                .0
                .lock()
                .expect("sidecar lock poisoned")
                .replace(child);

            // Drain the sidecar's output so its pipes never block; mirror to stdout.
            tauri::async_runtime::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => {
                            print!("[api] {}", String::from_utf8_lossy(&bytes));
                        }
                        CommandEvent::Error(err) => eprintln!("[api] error: {err}"),
                        _ => {}
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Tauri application")
        .run(|app_handle, event| {
            // Stop the API sidecar when the app exits.
            if let RunEvent::Exit = event {
                if let Some(child) = app_handle
                    .state::<ApiSidecar>()
                    .0
                    .lock()
                    .expect("sidecar lock poisoned")
                    .take()
                {
                    let _ = child.kill();
                }
            }
        });
}
