use crate::core::convert::{ConvertParams, ConvertResult, Progress};
use crate::core::probe::MediaInfo;
use crate::core::{convert as conv, probe, temp_dir, unique_id};
#[cfg(windows)]
use crate::core::ffmpeg;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, State};

#[derive(Default, Clone)]
pub struct JobState {
    pids: Arc<Mutex<HashMap<String, u32>>>,
    cancelled: Arc<Mutex<HashSet<String>>>,
}

/// Отравленный Mutex не должен ронять IPC-обработчики: данные примитивные,
/// восстанавливаем внутреннее состояние.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[tauri::command]
pub async fn probe_file(path: String) -> Result<MediaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || probe::probe(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}

/// Вставка из буфера: список файлов (CF_HDROP) или растровое изображение -> temp PNG.
#[cfg(windows)]
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

#[cfg(not(windows))]
#[tauri::command]
pub async fn clipboard_paste() -> Result<Vec<String>, String> {
    Err("Вставка из буфера недоступна на этой платформе".into())
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
    lock(&cancelled).remove(&id); // сброс перед повтором

    tauri::async_runtime::spawn_blocking(move || {
        let emit_id = id.clone();
        let mut emit = move |attempt: u32, pass: u32, pct: f64| {
            let _ = app.emit(
                "convert-progress",
                json!({ "id": emit_id, "attempt": attempt, "pass": pass, "pct": pct }),
            );
        };
        // PID снимается сразу после завершения процесса, чтобы отмена
        // не убила чужой процесс с переиспользованным PID
        let pid_id = id.clone();
        let pids_reg = pids.clone();
        let set_pid = move |pid: Option<u32>| {
            let mut map = lock(&pids_reg);
            match pid {
                Some(v) => {
                    map.insert(pid_id.clone(), v);
                }
                None => {
                    map.remove(&pid_id);
                }
            }
        };
        let cancel_id = id.clone();
        let cancelled_chk = cancelled.clone();
        let is_cancelled = move || lock(&cancelled_chk).contains(&cancel_id);

        let mut prog = Progress {
            emit: &mut emit,
            set_pid: &set_pid,
            is_cancelled: &is_cancelled,
        };
        let r = conv::convert(&params, &mut prog);
        lock(&pids).remove(&id);
        lock(&cancelled).remove(&id);
        r
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel(state: State<'_, JobState>, id: String) {
    lock(&state.cancelled).insert(id.clone());
    if let Some(pid) = lock(&state.pids).get(&id).copied() {
        crate::core::platform::kill_pid(pid);
    }
}

#[tauri::command]
pub async fn make_preview(input: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || conv::make_proxy(&input, |_| {}))
        .await
        .map_err(|e| e.to_string())?
}

/// Положить готовый файл в буфер обмена (CF_HDROP) — как «Копировать» в проводнике.
#[cfg(windows)]
#[tauri::command]
pub fn clipboard_copy_file(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err("Файл не найден".into());
    }
    use clipboard_win::{formats, Clipboard, Setter};
    let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Буфер обмена: {e}"))?;
    formats::FileList
        .write_clipboard(&[path][..])
        .map_err(|e| format!("Буфер обмена: {e}"))
}

#[cfg(not(windows))]
#[tauri::command]
pub fn clipboard_copy_file(_path: String) -> Result<(), String> {
    Err("Копирование файла недоступно на этой платформе".into())
}

/// Открыть проводник с выделенным файлом.
///
/// std::process::Command оборачивает аргумент с пробелами в кавычки целиком
/// (`"/select,C:\a b.webm"`) — explorer такое не парсит и открывает «Документы».
/// Поэтому аргумент собирается вручную через raw_arg: `/select,"C:\a b.webm"`.
#[tauri::command]
pub fn reveal(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("Файл не найден".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let native = path.replace('/', "\\");
        ffmpeg::cmd_raw("explorer")
            .raw_arg(format!("/select,\"{native}\""))
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Открытие папки недоступно на этой платформе".into())
    }
}

/// Windows: %APPDATA%\StickerNah\settings.json; если рядом с exe уже лежит
/// settings.json — используется он (портативный режим, включается вручную).
/// Android: каталог конфигурации приложения.
fn settings_path(app: &AppHandle) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        if let Ok(dir) = app.path().app_config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            return dir.join("settings.json");
        }
    }
    let _ = app;
    let near_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("settings.json")));
    if let Some(p) = &near_exe {
        if p.exists() {
            return p.clone();
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let dir = PathBuf::from(appdata).join("StickerNah");
        if std::fs::create_dir_all(&dir).is_ok() {
            return dir.join("settings.json");
        }
    }
    near_exe.unwrap_or_else(|| PathBuf::from("settings.json"))
}

#[tauri::command]
pub fn settings_load(app: AppHandle) -> String {
    std::fs::read_to_string(settings_path(&app)).unwrap_or_else(|_| "{}".into())
}

#[tauri::command]
pub fn settings_save(app: AppHandle, data: String) -> Result<(), String> {
    std::fs::write(settings_path(&app), data).map_err(|e| e.to_string())
}

/// Имя файла от ContentProvider/IPC — недоверенное: берётся только последний
/// компонент пути, служебные символы заменяются, длина ограничивается.
#[cfg(target_os = "android")]
fn sanitize_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = last
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    s = s.trim_start_matches('.').to_string();
    if s.is_empty() {
        return "import".into();
    }
    if s.chars().count() > 120 {
        // хвост важнее головы — там расширение
        s = s
            .chars()
            .rev()
            .take(120)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
    }
    s
}

/// Android: системный выбор файлов (SAF) отдаёт content://-URI, которые ffmpeg
/// не может открыть напрямую. Копируем содержимое через android_fs во
/// временный файл — дальше ядро работает с обычным путём как на Windows.
#[cfg(target_os = "android")]
#[tauri::command]
pub async fn android_pick_files(app: AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_android_fs::AndroidFsExt;
    let api = app.android_fs_async();
    let uris = api
        .file_picker()
        .pick_files(None, &["image/*", "video/*"], false)
        .await
        .map_err(|e| e.to_string())?;

    let mut paths = Vec::with_capacity(uris.len());
    for uri in uris {
        let name = sanitize_name(
            &api.get_name(&uri)
                .await
                .unwrap_or_else(|_| "import".to_string()),
        );
        // потоково, а не через Vec<u8>: крупное видео не должно исчерпать память
        let mut src = api
            .open_file_readable(&uri)
            .await
            .map_err(|e| e.to_string())?;
        let dest = temp_dir().join(format!("import_{}_{name}", unique_id()));
        let mut dst = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut src, &mut dst).map_err(|e| e.to_string())?;
        paths.push(dest.to_string_lossy().into_owned());
    }
    Ok(paths)
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn android_pick_files(_app: AppHandle) -> Result<Vec<String>, String> {
    Err("Доступно только на Android".into())
}

/// Android: готовый стикер сохраняется в общедоступную галерею —
/// Movies/Sticker-Nah/<имя>.webm — чтобы им можно было поделиться из Telegram
/// и других приложений (выбор папки вывода на Android не поддерживается).
#[cfg(target_os = "android")]
#[tauri::command]
pub async fn android_save_to_gallery(
    app: AppHandle,
    path: String,
    filename: String,
) -> Result<(), String> {
    use tauri_plugin_android_fs::{AndroidFsExt, PublicVideoDir};
    let filename = sanitize_name(&filename);
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let api = app.android_fs_async();
    let storage = api.public_storage();
    if !storage.check_permission().await.unwrap_or(false) {
        storage.request_permission().await.map_err(|e| e.to_string())?;
    }
    storage
        .write_new(
            None,
            PublicVideoDir::Movies,
            format!("Sticker-Nah/{filename}"),
            Some("video/webm"),
            &bytes,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn android_save_to_gallery(
    _app: AppHandle,
    _path: String,
    _filename: String,
) -> Result<(), String> {
    Err("Доступно только на Android".into())
}
