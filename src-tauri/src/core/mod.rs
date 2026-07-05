pub mod convert;
pub mod ffmpeg;
pub mod platform;
pub mod probe;

use std::path::PathBuf;

/// Временная папка программы внутри %TEMP%.
pub fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join("sticker-nah");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn unique_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
