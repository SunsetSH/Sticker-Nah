use crate::core::convert::{ConvertParams, ConvertResult, Progress};
use crate::core::probe::MediaInfo;
use crate::core::{convert as conv, ffmpeg, probe, temp_dir, unique_id};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

#[derive(Default, Clone)]
pub struct JobState {
    pids: Arc<Mutex<HashMap<String, u32>>>,
    cancelled: Arc<Mutex<HashSet<String>>>,
}

#[tauri::command]
pub async fn probe_file(path: String) -> Result<MediaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || probe::probe(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}

/// Вставка из буфера: список файлов (CF_HDROP) или растровое изображение -> temp PNG.
#[tauri::command]
pub async fn clipboard_paste() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        if let Ok(list) =
            clipboard_win::get_clipboard::<Vec<String>, _>(clipboard_win::formats::FileList)
        {
            if !list.is_empty() {
                return Ok(list);
            }
        }
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        match cb.get_image() {
            Ok(img) => {
                let buf = image::RgbaImage::from_raw(
                    img.width as u32,
                    img.height as u32,
                    img.bytes.into_owned(),
                )
                .ok_or("Не удалось прочитать изображение из буфера")?;
                let path = temp_dir().join(format!("paste_{}.png", unique_id()));
                buf.save(&path).map_err(|e| e.to_string())?;
                Ok(vec![path.to_string_lossy().into_owned()])
            }
            Err(_) => Err("В буфере обмена нет файлов или изображения".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn convert(
    app: AppHandle,
    state: State<'_, JobState>,
    id: String,
    params: ConvertParams,
) -> Result<ConvertResult, String> {
    let pids = state.pids.clone();
    let cancelled = state.cancelled.clone();
    cancelled.lock().unwrap().remove(&id); // сброс перед повтором

    let res = tauri::async_runtime::spawn_blocking(move || {
        let emit_id = id.clone();
        let mut emit = move |attempt: u32, pass: u32, pct: f64| {
            let _ = app.emit(
                "convert-progress",
                json!({ "id": emit_id, "attempt": attempt, "pass": pass, "pct": pct }),
            );
        };
        let pid_id = id.clone();
        let pids_reg = pids.clone();
        let register_pid = move |pid: u32| {
            pids_reg.lock().unwrap().insert(pid_id.clone(), pid);
        };
        let cancel_id = id.clone();
        let cancelled_chk = cancelled.clone();
        let is_cancelled = move || cancelled_chk.lock().unwrap().contains(&cancel_id);

        let mut prog = Progress {
            emit: &mut emit,
            register_pid: &register_pid,
            is_cancelled: &is_cancelled,
        };
        let r = conv::convert(&params, &mut prog);
        pids.lock().unwrap().remove(&id);
        r
    })
    .await
    .map_err(|e| e.to_string())?;
    res
}

#[tauri::command]
pub fn cancel(state: State<'_, JobState>, id: String) {
    state.cancelled.lock().unwrap().insert(id.clone());
    if let Some(pid) = state.pids.lock().unwrap().get(&id).copied() {
        let _ = ffmpeg::cmd_raw("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

#[tauri::command]
pub async fn make_preview(input: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || conv::make_proxy(&input, |_| {}))
        .await
        .map_err(|e| e.to_string())?
}

/// Открыть проводник с выделенным файлом.
#[tauri::command]
pub fn reveal(path: String) -> Result<(), String> {
    ffmpeg::cmd_raw("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn settings_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("settings.json")))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

#[tauri::command]
pub fn settings_load() -> String {
    std::fs::read_to_string(settings_path()).unwrap_or_else(|_| "{}".into())
}

#[tauri::command]
pub fn settings_save(data: String) -> Result<(), String> {
    std::fs::write(settings_path(), data).map_err(|e| e.to_string())
}
