#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod core;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::JobState::default())
        .invoke_handler(tauri::generate_handler![
            commands::probe_file,
            commands::clipboard_paste,
            commands::convert,
            commands::cancel,
            commands::make_preview,
            commands::reveal,
            commands::settings_load,
            commands::settings_save,
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска Sticker Nah");
}
