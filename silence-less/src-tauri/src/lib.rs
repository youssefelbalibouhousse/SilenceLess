//! Backend de l'application de bureau SilenceLess (Tauri v2).
//!
//! Expose des commandes appelées par l'interface (HTML/JS) et envoie des
//! événements de progression. Le traitement audio est délégué à
//! `silence-less-core` dans des threads de travail, pour ne pas bloquer l'UI.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use silence_less_core::{process_file, Settings as CoreSettings};
use tauri::{Emitter, Manager};

/// Réglages transmis par l'interface (les noms de champs sont en camelCase).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiSettings {
    threshold_db: f64,
    min_silence_ms: u32,
    padding_ms: u32,
    seek_step_ms: u32,
    bitrate_kbps: u32,
    trim_internal: bool,
}

impl UiSettings {
    fn into_core(self) -> CoreSettings {
        CoreSettings {
            silence_threshold_db: self.threshold_db,
            min_silence_len_ms: self.min_silence_ms,
            keep_padding_ms: self.padding_ms,
            seek_step_ms: self.seek_step_ms,
            bitrate_kbps: self.bitrate_kbps,
            trim_internal_silence: self.trim_internal,
        }
    }
}

/// Progression d'un fichier, émise vers l'interface.
#[derive(Serialize, Clone)]
struct ProgressPayload {
    done: usize,
    total: usize,
    name: String,
    ok: bool,
    message: String,
}

/// Résumé final.
#[derive(Serialize, Clone)]
struct DonePayload {
    total: usize,
    failures: usize,
}

/// Traite une liste de fichiers dans un thread de travail, en émettant la
/// progression au fil de l'eau.
fn process_paths(
    app: tauri::AppHandle,
    inputs: Vec<PathBuf>,
    out_dir: PathBuf,
    settings: CoreSettings,
) {
    let total = inputs.len();
    std::thread::spawn(move || {
        let mut failures = 0_usize;
        for (index, wav) in inputs.iter().enumerate() {
            let name = wav
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stem = wav
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mp3 = out_dir.join(format!("{stem}.mp3"));

            let payload = match process_file(wav, &mp3, &settings) {
                Ok(stats) if stats.all_silent => ProgressPayload {
                    done: index + 1,
                    total,
                    name,
                    ok: true,
                    message: "entièrement silencieux, ignoré".to_string(),
                },
                Ok(stats) => ProgressPayload {
                    done: index + 1,
                    total,
                    name,
                    ok: true,
                    message: format!("{:.2}s", stats.kept_ms as f64 / 1000.0),
                },
                Err(e) => {
                    failures += 1;
                    ProgressPayload {
                        done: index + 1,
                        total,
                        name,
                        ok: false,
                        message: e,
                    }
                }
            };
            let _ = app.emit("silence-progress", payload);
        }
        let _ = app.emit("silence-done", DonePayload { total, failures });
    });
}

#[tauri::command]
fn process_folder(
    app: tauri::AppHandle,
    folder: String,
    settings: UiSettings,
    out: Option<String>,
) -> Result<(), String> {
    let in_dir = PathBuf::from(&folder);
    if !in_dir.is_dir() {
        return Err(format!("dossier introuvable : {folder}"));
    }
    let out_dir = match out {
        Some(o) if !o.trim().is_empty() => PathBuf::from(o),
        _ => in_dir.join("mp3_output"),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("sortie : {e}"))?;

    let wavs = silence_less_core::find_wav_files(&in_dir);
    if wavs.is_empty() {
        return Err("aucun fichier .wav trouvé dans ce dossier".to_string());
    }

    process_paths(app, wavs, out_dir, settings.into_core());
    Ok(())
}

#[tauri::command]
fn process_files(
    app: tauri::AppHandle,
    paths: Vec<String>,
    settings: UiSettings,
    out: Option<String>,
) -> Result<(), String> {
    let wavs: Vec<PathBuf> = paths
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();
    if wavs.is_empty() {
        return Err("aucun fichier .wav valide".to_string());
    }

    let out_dir = match out {
        Some(o) if !o.trim().is_empty() => PathBuf::from(o),
        _ => wavs[0]
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("mp3_output"),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("sortie : {e}"))?;

    process_paths(app, wavs, out_dir, settings.into_core());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![process_folder, process_files])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::DragDrop(drag) = event {
                        match drag {
                            tauri::DragDropEvent::Enter { .. }
                            | tauri::DragDropEvent::Over { .. } => {
                                let _ = handle.emit("drop-active", true);
                            }
                            tauri::DragDropEvent::Leave => {
                                let _ = handle.emit("drop-active", false);
                            }
                            tauri::DragDropEvent::Drop { paths, .. } => {
                                let _ = handle.emit("drop-active", false);
                                let strings: Vec<String> =
                                    paths.iter().map(|p| p.display().to_string()).collect();
                                let _ = handle.emit("drop-paths", strings);
                            }
                            _ => {}
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application Tauri");
}
