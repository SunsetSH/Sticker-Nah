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
    use std::sync::OnceLock;
    // проверка целостности читает весь файл — успешный результат кэшируем на
    // процесс; неудачу (антивирус держит файл, диск переполнен и т.п.) не
    // кэшируем, чтобы следующий вызов повторил попытку, а не ломал ffmpeg
    // до перезапуска приложения
    static FFMPEG_PATH: OnceLock<PathBuf> = OnceLock::new();
    static FFPROBE_PATH: OnceLock<PathBuf> = OnceLock::new();
    let cache = match name {
        "ffmpeg" => &FFMPEG_PATH,
        "ffprobe" => &FFPROBE_PATH,
        _ => return None,
    };
    if let Some(p) = cache.get() {
        return Some(p.clone());
    }
    let path = extract_tool(name)?;
    // set() может проиграть гонку другому потоку — в этом случае просто
    // используем то значение, что уже там (оно указывает на тот же файл)
    Some(cache.get_or_init(|| path.clone()).clone())
}

#[cfg(windows)]
fn extract_tool(name: &str) -> Option<PathBuf> {
    let bytes: &[u8] = match name {
        "ffmpeg" => embedded::FFMPEG,
        "ffprobe" => embedded::FFPROBE,
        _ => return None,
    };
    // побайтовое сравнение с встроенной копией: длины недостаточно —
    // повреждённый или подменённый файл той же длины прошёл бы проверку
    let matches_embedded =
        |p: &PathBuf| std::fs::read(p).map(|d| d == bytes).unwrap_or(false);
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)?
        .join("StickerNah")
        .join("bin")
        .join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&dir).ok()?;
    hold_version_lock(&dir);
    let path = dir.join(format!("{name}.exe"));
    if !matches_embedded(&path) {
        // распаковка через временный файл + rename, безопасно при гонке
        let tmp = dir.join(format!("{name}.{}.tmp", super::unique_id()));
        std::fs::write(&tmp, bytes).ok()?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            // rename мог проиграть гонку другому процессу — верить файлу
            // можно только после повторной проверки содержимого
            if !matches_embedded(&path) {
                return None;
            }
        }
    }
    Some(path)
}

/// Держит эксклюзивный лок на каталог версии, пока процесс жив: по нему
/// cleanup_old_extracted в другом запущенном экземпляре понимает, что версия
/// используется, и не удаляет её бинарники из-под работающего процесса.
#[cfg(windows)]
fn hold_version_lock(dir: &std::path::Path) {
    use std::fs::File;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::OnceLock;
    static LOCK: OnceLock<Option<File>> = OnceLock::new();
    LOCK.get_or_init(|| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(dir.join(".lock"))
            .ok()
    });
}

/// true, если каталог версии заблокирован работающим экземпляром приложения
/// (hold_version_lock держит .lock без общего доступа — открыть его отсюда
/// не получится, пока тот процесс жив).
#[cfg(windows)]
fn dir_in_use(dir: &std::path::Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(dir.join(".lock"))
        .is_err()
}

/// Удаление распакованных бинарей прежних версий приложения.
#[cfg(windows)]
pub fn cleanup_old_extracted() {
    let Some(bin) = std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("StickerNah").join("bin"))
    else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&bin) else {
        return;
    };
    for entry in rd.flatten() {
        if entry.file_name() != env!("CARGO_PKG_VERSION") && !dir_in_use(&entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(not(windows))]
pub fn cleanup_old_extracted() {}

/// Завершить дерево процессов по PID. false — сигнал послать не удалось
/// (процесс всё равно остановится на ближайшей проверке is_cancelled).
pub fn kill_pid(pid: u32) -> bool {
    #[cfg(windows)]
    let r = super::ffmpeg::cmd_raw("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
    #[cfg(not(windows))]
    let r = super::ffmpeg::cmd_raw("kill")
        .args(["-9", &pid.to_string()])
        .output();
    r.map(|o| o.status.success()).unwrap_or(false)
}
