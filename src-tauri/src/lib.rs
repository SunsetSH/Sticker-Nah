mod commands;
mod core;

/// Временные файлы (вставки из буфера, прокси-превью, хвосты pass-логов)
/// старше 3 суток и распакованные бинари прежних версий удаляются при старте.
fn cleanup_old_temp() {
    std::thread::spawn(|| {
        core::platform::cleanup_old_extracted();
        let ttl = std::time::Duration::from_secs(3 * 24 * 3600);
        let Ok(rd) = std::fs::read_dir(core::temp_dir()) else {
            return;
        };
        for entry in rd.flatten() {
            let expired = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|age| age > ttl)
                .unwrap_or(false);
            if expired {
                let p = entry.path();
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // android-fs нужен только на Android — на десктопе не тащим зависимость
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_android_fs::init());
    builder
        .setup(|_app| {
            // Android: std::env::temp_dir() указывает на недоступный /data/local/tmp —
            // перенаправляем во внутренний кэш приложения
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                if let Ok(dir) = _app.path().app_cache_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    std::env::set_var("TMPDIR", &dir);
                }
            }
            cleanup_old_temp();
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::JobState::default())
        .invoke_handler(tauri::generate_handler![
            commands::probe_file,
            commands::clipboard_paste,
            commands::clipboard_copy_file,
            commands::convert,
            commands::cancel,
            commands::make_preview,
            commands::reveal,
            commands::settings_load,
            commands::settings_save,
            commands::android_pick_files,
            commands::android_save_to_gallery,
            commands::android_open_file,
            commands::android_share_file,
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска Sticker Nah");
}
