#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod startup;

use proxy_vouch_core::{
    export::{self, ExportOptions, Payload},
    model::{AppError, AppResult, CheckSettings},
    parser::{ImportOptions, MAX_BYTES},
    session::{self, Preview, SharedSession, Snapshot},
    storage::{
        Backup, BackupPreview, BackupScope, Preferences, RestoreMode, RestoreResult, StorageStatus,
        Store,
    },
};
use serde::Serialize;
use std::{
    io::Read,
    sync::{Arc, Mutex},
};
use tauri::{Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
fn snapshot(state: State<'_, SharedSession>, since: u64) -> AppResult<Snapshot> {
    Ok(session::lock(&state)?.snapshot(since))
}

#[tauri::command]
async fn preview_import(
    state: State<'_, SharedSession>,
    text: String,
    options: ImportOptions,
) -> AppResult<Preview> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || session::lock(&state)?.preview(&text, &options))
        .await
        .map_err(|_| AppError::new("INTERNAL_ERROR", "Import worker failed."))?
}

#[tauri::command]
fn commit_import(
    state: State<'_, SharedSession>,
    replace: bool,
    keep_duplicates: bool,
    include_invalid: bool,
) -> AppResult<usize> {
    session::lock(&state)?.commit_import(replace, keep_duplicates, include_invalid)
}

#[tauri::command]
fn start_check(
    state: State<'_, SharedSession>,
    ids: Vec<u64>,
    settings: CheckSettings,
    detect_again: bool,
) -> AppResult<u64> {
    session::start(Arc::clone(&state), ids, settings, detect_again)
}

#[tauri::command]
fn stop_check(state: State<'_, SharedSession>) -> AppResult<()> {
    if let Some(control) = &session::lock(&state)?.control {
        control.cancel();
    }
    Ok(())
}

#[tauri::command]
fn clear_entries(state: State<'_, SharedSession>, ids: Vec<u64>) -> AppResult<()> {
    session::lock(&state)?.clear(&ids)
}

#[tauri::command]
fn edit_entry(state: State<'_, SharedSession>, id: u64, text: String) -> AppResult<()> {
    session::lock(&state)?.edit(id, &text)
}

#[tauri::command]
fn reveal_entry(state: State<'_, SharedSession>, id: u64) -> AppResult<String> {
    let state = session::lock(&state)?;
    let entry = state
        .entries
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| AppError::new("NOT_FOUND", "Record no longer exists."))?;
    Ok(entry.parsed.raw.clone())
}

#[tauri::command]
fn read_clipboard(app: tauri::AppHandle) -> AppResult<String> {
    app.clipboard()
        .read_text()
        .map_err(|_| AppError::new("CLIPBOARD_ERROR", "Could not read text from the clipboard."))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFile {
    text: String,
    source_name: String,
}

#[tauri::command]
async fn import_file(app: tauri::AppHandle) -> AppResult<Option<ImportedFile>> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Proxy lists", &["txt", "csv", "tsv"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file
            .into_path()
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Select a local file."))?;
        let file = std::fs::File::open(&path)
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Cannot read the selected file."))?;
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Could not read the complete file."))?;
        if bytes.len() > MAX_BYTES {
            return Err(AppError::new(
                "IMPORT_TOO_LARGE",
                "Import must not exceed 20 MiB.",
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| AppError::new("INVALID_ENCODING", "The file must be encoded as UTF-8."))?;
        Ok(Some(ImportedFile {
            text,
            source_name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported file".into()),
        }))
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR", "File dialog worker failed."))?
}

#[tauri::command]
async fn export_data(
    app: tauri::AppHandle,
    state: State<'_, SharedSession>,
    options: ExportOptions,
    destination: String,
) -> AppResult<Option<usize>> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let Payload {
            text,
            count,
            extension,
        } = export::render(&session::lock(&state)?.entries, &options)?;
        match destination.as_str() {
            "clipboard" => app.clipboard().write_text(text).map_err(|_| {
                AppError::new("CLIPBOARD_ERROR", "Could not write to the clipboard.")
            })?,
            "file" => {
                let Some(file) = app
                    .dialog()
                    .file()
                    .set_file_name(format!(
                        "proxy-vouch-{}.{}",
                        options.scope.to_lowercase(),
                        extension
                    ))
                    .add_filter("Proxy export", &[&extension])
                    .blocking_save_file()
                else {
                    return Ok(None);
                };
                let path = file.into_path().map_err(|_| {
                    AppError::new("FILE_WRITE_FAILED", "Select a local output file.")
                })?;
                export::save_atomic(&path, &text)?;
            }
            _ => return Err(AppError::new("INVALID_EXPORT", "Choose clipboard or file.")),
        }
        Ok(Some(count))
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR", "Export worker failed."))?
}

type SharedStore = Arc<Store>;
type PendingBackup = Arc<Mutex<Option<Backup>>>;

#[tauri::command]
fn load_preferences(state: State<'_, SharedSession>) -> AppResult<Preferences> {
    Ok(session::lock(&state)?.preferences.clone())
}

#[tauri::command]
async fn save_preferences(
    state: State<'_, SharedSession>,
    store: State<'_, SharedStore>,
    preferences: Preferences,
) -> AppResult<()> {
    let state = Arc::clone(&state);
    let store = Arc::clone(&store);
    tauri::async_runtime::spawn_blocking(move || store.set_preferences(&state, preferences))
        .await
        .map_err(|_| AppError::new("STORAGE_ERROR", "The save worker failed."))?
}

#[tauri::command]
async fn storage_status(store: State<'_, SharedStore>) -> AppResult<StorageStatus> {
    let store = Arc::clone(&store);
    tauri::async_runtime::spawn_blocking(move || store.status())
        .await
        .map_err(|_| AppError::new("STORAGE_ERROR", "The storage worker failed."))?
}

#[tauri::command]
async fn flush_workspace(
    state: State<'_, SharedSession>,
    store: State<'_, SharedStore>,
) -> AppResult<()> {
    let state = Arc::clone(&state);
    let store = Arc::clone(&store);
    tauri::async_runtime::spawn_blocking(move || store.save(&state))
        .await
        .map_err(|_| AppError::new("STORAGE_ERROR", "The save worker failed."))?
}

#[tauri::command]
async fn export_backup(
    app: tauri::AppHandle,
    state: State<'_, SharedSession>,
    scope: BackupScope,
) -> AppResult<bool> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let backup = Backup::capture(&*session::lock(&state)?, scope);
        let Some(file) = app
            .dialog()
            .file()
            .set_file_name("proxy-vouch-backup.json")
            .add_filter("ProxyVouch backup", &["json"])
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = file
            .into_path()
            .map_err(|_| AppError::new("FILE_WRITE_FAILED", "Select a local output file."))?;
        if let (Ok(parent), Ok(directory)) = (
            path.parent()
                .unwrap_or(std::path::Path::new("."))
                .canonicalize(),
            app.path()
                .app_data_dir()
                .and_then(|p| p.canonicalize().map_err(Into::into)),
        ) {
            if parent.starts_with(directory) {
                return Err(AppError::new(
                    "INVALID_EXPORT",
                    "Choose a backup location outside the application data folder.",
                ));
            }
        }
        export::save_atomic(&path, &backup.encode()?)?;
        Ok(true)
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR", "The backup worker failed."))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupFilePreview {
    source_name: String,
    summary: BackupPreview,
}

#[tauri::command]
async fn preview_backup(
    app: tauri::AppHandle,
    pending: State<'_, PendingBackup>,
) -> AppResult<Option<BackupFilePreview>> {
    let pending = Arc::clone(&pending);
    tauri::async_runtime::spawn_blocking(move || {
        *pending
            .lock()
            .map_err(|_| AppError::new("INTERNAL_ERROR", "Backup preview unavailable."))? = None;
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("ProxyVouch backup", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file
            .into_path()
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Select a local file."))?;
        let backup = Backup::read(&path)?;
        let preview = BackupFilePreview {
            source_name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            summary: backup.preview(),
        };
        *pending
            .lock()
            .map_err(|_| AppError::new("INTERNAL_ERROR", "Backup preview unavailable."))? =
            Some(backup);
        Ok(Some(preview))
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR", "The backup worker failed."))?
}

#[tauri::command]
async fn restore_backup(
    state: State<'_, SharedSession>,
    store: State<'_, SharedStore>,
    pending: State<'_, PendingBackup>,
    mode: RestoreMode,
    settings: bool,
) -> AppResult<RestoreResult> {
    let state = Arc::clone(&state);
    let store = Arc::clone(&store);
    let pending = Arc::clone(&pending);
    tauri::async_runtime::spawn_blocking(move || {
        let mut pending = pending
            .lock()
            .map_err(|_| AppError::new("INTERNAL_ERROR", "Backup preview unavailable."))?;
        let backup = pending
            .as_ref()
            .ok_or_else(|| AppError::new("NO_PREVIEW", "Choose a backup file first."))?;
        let result = store.restore(&state, backup, mode, settings)?;
        *pending = None;
        Ok(result)
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR", "The restore worker failed."))?
}

fn main() {
    // Environment changes must precede every runtime, plugin and worker thread.
    startup::configure_before_runtime();
    let result = tauri::Builder::default()
        .manage(PendingBackup::default())
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            let legacy = app
                .path()
                .app_config_dir()
                .ok()
                .map(|p| p.join("preferences.json"));
            let (store, restored) = Store::open(directory, legacy.as_deref());
            let store = Arc::new(store);
            let state: SharedSession = Arc::new(Mutex::new(restored));
            app.manage(Arc::clone(&state));
            app.manage(Arc::clone(&store));
            let state = Arc::downgrade(&state);
            let store = Arc::downgrade(&store);
            std::thread::spawn(move || loop {
                let (Some(state), Some(store)) = (state.upgrade(), store.upgrade()) else {
                    break;
                };
                // Errors remain visible through storage_status; only one writer can run.
                let _ = store.save(&state);
                drop(state);
                drop(store);
                std::thread::sleep(std::time::Duration::from_secs(1));
            });
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            preview_import,
            commit_import,
            start_check,
            stop_check,
            clear_entries,
            edit_entry,
            reveal_entry,
            read_clipboard,
            import_file,
            export_data,
            load_preferences,
            save_preferences,
            storage_status,
            flush_workspace,
            export_backup,
            preview_backup,
            restore_backup
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Ok(state) = session::lock(&window.state::<SharedSession>()) {
                    if let Some(control) = &state.control {
                        control.cancel();
                    }
                }
                let _ = window
                    .state::<SharedStore>()
                    .save(&window.state::<SharedSession>());
            }
        })
        .run(tauri::generate_context!());
    if result.is_err() {
        eprintln!("ProxyVouch could not start. Verify the desktop runtime dependencies.");
        std::process::exit(1);
    }
}
