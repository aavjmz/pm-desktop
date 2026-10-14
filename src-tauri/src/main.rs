// Release builds start without a console window; debug builds keep one for logging.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Desktop host for 项目里程碑指挥台.
//!
//! The host deliberately does very little: it opens one window, serves the existing
//! single-file app, and exposes the local document store to it. All the project-management
//! logic stays in the page, unchanged from the web build, so the two cannot drift apart.

use std::sync::Mutex;

use milestone_core::paths::{self, DataLocation};
use milestone_core::store::Store;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

struct AppState {
    store: Store,
    location: DataLocation,
}

#[derive(Serialize)]
struct StorageInfo {
    path: String,
    /// "portable" when data sits beside the exe, "appdata" when it fell back.
    location: &'static str,
    backups_kept: usize,
}

#[tauri::command]
fn storage_get(key: String, state: State<'_, Mutex<AppState>>) -> Result<Option<String>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.store.get(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn storage_set(
    key: String,
    value: String,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.store.set(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
fn storage_delete(key: String, state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.store.delete(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn storage_list(state: State<'_, Mutex<AppState>>) -> Result<Vec<String>, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    s.store.list().map_err(|e| e.to_string())
}

/// Where the data actually ended up — shown in the UI so the person is never guessing.
#[tauri::command]
fn storage_info(state: State<'_, Mutex<AppState>>) -> Result<StorageInfo, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    Ok(StorageInfo {
        path: s.store.root().to_string_lossy().to_string(),
        location: match s.location {
            DataLocation::BesideExe => "portable",
            DataLocation::AppData => "appdata",
        },
        backups_kept: milestone_core::store::BACKUP_KEEP,
    })
}

/// Save bytes to a location the user picks, through the native Windows save dialog.
///
/// The web build hands files to the browser's download machinery (`XLSX.writeFile`, or a Blob
/// plus a synthetic anchor click). Inside an application webview that path is at best a
/// silent write to the Downloads folder and at worst blocked outright, with no way for the
/// person to choose where the file goes — so exports go through the real dialog instead.
///
/// Returns the chosen path, or `None` when the user cancels (which is not an error).
#[tauri::command]
async fn save_file(
    app: AppHandle,
    suggested_name: String,
    bytes: Vec<u8>,
) -> Result<Option<String>, String> {
    let name = suggested_name.clone();
    let ext = std::path::Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();

    let mut builder = app.dialog().file().set_file_name(&name);
    if !ext.is_empty() {
        let label = match ext.as_str() {
            "xlsx" => "Excel 工作簿",
            "json" => "备份文件",
            _ => "文件",
        };
        builder = builder.add_filter(label, &[ext.as_str()]);
    }

    // Runs on a worker thread because the command is async, so the window keeps painting.
    let picked = builder.blocking_save_file();
    let Some(path) = picked else { return Ok(None) };
    let path = path
        .into_path()
        .map_err(|e| format!("无法解析保存路径：{}", e))?;

    std::fs::write(&path, &bytes).map_err(|e| format!("写入文件失败：{}", e))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// Point WebView2 at the runtime shipped inside the app folder.
///
/// The portable build carries a fixed WebView2 version so it runs on a locked-down machine
/// that has no Evergreen runtime and no way to install one. Setting this before any window is
/// created makes the loader use our copy; if the folder is absent (a developer build, say) the
/// variable is left alone and WebView2 falls back to whatever is installed system-wide.
#[cfg(target_os = "windows")]
fn use_bundled_webview2() {
    if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_some() {
        return; // respect an explicit override
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("webview2");
            if bundled.join("msedgewebview2.exe").exists() {
                std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &bundled);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn use_bundled_webview2() {}

fn main() {
    use_bundled_webview2();

    let data = paths::resolve();
    let store = match Store::open(&data.path) {
        Ok(s) => s,
        Err(e) => {
            // Without a store there is nothing to run; say so in the language of the app.
            eprintln!("无法初始化数据目录 {}：{}", data.path.display(), e);
            std::process::exit(1);
        }
    };
    let state = Mutex::new(AppState {
        store,
        location: data.location,
    });

    tauri::Builder::default()
        // A second launch focuses the existing window instead of opening a rival one — two
        // windows writing the same JSON file would quietly overwrite each other's work.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            storage_get,
            storage_set,
            storage_delete,
            storage_list,
            storage_info,
            save_file
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
