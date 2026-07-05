//! Платформенный слой: всё, что различается между Windows и Android,
//! собрано здесь, чтобы ядро оставалось общим.

use std::path::PathBuf;

/// Приёмник вывода для первого прохода (`-f null <sink>`).
pub fn null_sink() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

/// Имя исполняемого файла инструмента на платформе.
#[cfg(not(target_os = "android"))]
pub fn tool_file(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

/// Каталоги, в которых ищем ffmpeg/ffprobe (по порядку приоритета).
#[cfg(not(target_os = "android"))]
pub fn tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(cur) = std::env::current_exe() {
        if let Some(dir) = cur.parent() {
            dirs.push(dir.join("bin"));
            dirs.push(dir.join("bin").join("win"));
            dirs.push(dir.to_path_buf());
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin").join("win"));
    dirs
}

/// Android: бинарники лежат как lib{name}.so в nativeLibraryDir приложения.
/// Путь к нему находим по загруженной библиотеке процесса в /proc/self/maps.
#[cfg(target_os = "android")]
pub fn tool_file(name: &str) -> String {
    format!("lib{name}.so")
}

#[cfg(target_os = "android")]
pub fn tool_dirs() -> Vec<PathBuf> {
    use std::io::BufRead;
    let mut dirs = Vec::new();
    if let Ok(f) = std::fs::File::open("/proc/self/maps") {
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(pos) = line.find('/') {
                let p = &line[pos..];
                if p.contains("/lib/") && p.ends_with(".so") {
                    if let Some(dir) = std::path::Path::new(p).parent() {
                        let d = dir.to_path_buf();
                        if !dirs.contains(&d) {
                            dirs.push(d);
                        }
                    }
                }
            }
        }
    }
    dirs
}

/// Windows: ffmpeg/ffprobe встроены в exe и распаковываются в
/// %LOCALAPPDATA%\StickerNah\bin\<версия> при первом обращении —
/// приложение распространяется одним файлом.
#[cfg(windows)]
mod embedded {
    pub static FFMPEG: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/bin/win/ffmpeg.exe"));
    pub static FFPROBE: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/bin/win/ffprobe.exe"));
}

#[cfg(windows)]
pub fn extracted_tool(name: &str) -> Option<PathBuf> {
    let bytes: &[u8] = match name {
        "ffmpeg" => embedded::FFMPEG,
        "ffprobe" => embedded::FFPROBE,
        _ => return None,
    };
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)?
        .join("StickerNah")
        .join("bin")
        .join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.exe"));
    let valid = std::fs::metadata(&path)
        .map(|m| m.len() == bytes.len() as u64)
        .unwrap_or(false);
    if !valid {
        // распаковка через временный файл + rename, безопасно при гонке
        let tmp = dir.join(format!("{name}.{}.tmp", super::unique_id()));
        std::fs::write(&tmp, bytes).ok()?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            if !path.exists() {
                return None;
            }
        }
    }
    Some(path)
}

/// Завершить дерево процессов по PID.
pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = super::ffmpeg::cmd_raw("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = super::ffmpeg::cmd_raw("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}
