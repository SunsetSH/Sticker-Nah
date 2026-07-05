use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Ищет ffmpeg/ffprobe в платформенных каталогах (см. platform::tool_dirs),
/// затем в PATH.
pub fn tool_path(name: &str) -> PathBuf {
    let file = super::platform::tool_file(name);
    // внешние бинарники (рядом с exe / dev-папка) имеют приоритет —
    // это позволяет подменить встроенную версию
    for dir in super::platform::tool_dirs() {
        let c = dir.join(&file);
        if c.exists() {
            return c;
        }
    }
    #[cfg(windows)]
    if let Some(p) = super::platform::extracted_tool(name) {
        return p;
    }
    PathBuf::from(name)
}

pub fn cmd(tool: &str) -> Command {
    cmd_at(tool_path(tool))
}

/// Команда без поиска в bin/ (системные утилиты: taskkill, explorer).
pub fn cmd_raw(program: &str) -> Command {
    cmd_at(PathBuf::from(program))
}

fn cmd_at(path: PathBuf) -> Command {
    let mut c = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Запускает ffmpeg, читает `-progress pipe:1`, зовёт `on_progress` (0..1),
/// возвращает Err с хвостом stderr при неуспехе.
pub fn run_ffmpeg(
    args: &[String],
    duration: f64,
    mut on_progress: impl FnMut(f64),
    register_pid: impl FnOnce(u32),
) -> Result<(), String> {
    let mut child = cmd("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
    register_pid(child.id());

    let stderr = child
        .stderr
        .take()
        .ok_or("ffmpeg: поток stderr недоступен")?;
    let err_reader = std::thread::spawn(move || {
        let mut tail: VecDeque<String> = VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() >= 30 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let stdout = child
        .stdout
        .take()
        .ok_or("ffmpeg: поток stdout недоступен")?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let us = line
            .strip_prefix("out_time_us=")
            .or_else(|| line.strip_prefix("out_time_ms="));
        if let Some(v) = us {
            if let Ok(t) = v.trim().parse::<f64>() {
                if duration > 0.0 {
                    on_progress((t / 1_000_000.0 / duration).clamp(0.0, 1.0));
                }
            }
        }
    }

    let status = child.wait().map_err(|e| format!("ffmpeg wait: {e}"))?;
    let err_tail = err_reader.join().unwrap_or_default();
    if !status.success() {
        let tail: String = err_tail
            .chars()
            .rev()
            .take(1500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(if tail.is_empty() {
            format!("ffmpeg завершился с кодом {:?}", status.code())
        } else {
            tail
        });
    }
    Ok(())
}
