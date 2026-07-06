use super::ffmpeg::cmd;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Serialize, Clone, Debug)]
pub struct MediaInfo {
    pub kind: String, // "image" | "anim" | "video"
    pub width: u32,
    pub height: u32,
    pub duration: f64, // сек; 0 для статики
    pub fps: f64,
    pub size_bytes: u64,
    pub vcodec: String,
    pub format_name: String,
    pub has_alpha: bool,
    pub has_audio: bool,
    /// true, если WebView2 воспроизведёт файл напрямую (без прокси)
    pub browser_playable: bool,
}

fn parse_rate(s: &str) -> f64 {
    let mut it = s.split('/');
    let num: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let den: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    if den > 0.0 && num > 0.0 {
        num / den
    } else {
        0.0
    }
}

const ALPHA_PIX_FMTS: &[&str] = &[
    "rgba",
    "bgra",
    "argb",
    "abgr",
    "ya8",
    "ya16le",
    "ya16be",
    "yuva420p",
    "yuva422p",
    "yuva444p",
    "yuva420p10le",
    "yuva420p12le",
    "yuva422p10le",
    "yuva422p12le",
    "yuva444p10le",
    "yuva444p12le",
    "yuva444p16le",
    "gbrap",
    "gbrap10le",
    "gbrap12le",
    "gbrap16le",
    "rgba64le",
    "rgba64be",
];

const IMAGE_FORMATS: &[&str] = &[
    "image2",
    "png_pipe",
    "jpeg_pipe",
    "webp_pipe",
    "bmp_pipe",
    "tiff_pipe",
];

pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let size_bytes = std::fs::metadata(path)
        .map_err(|e| format!("Файл недоступен: {e}"))?
        .len();

    let out = cmd("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("Не удалось запустить ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Файл не распознан: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;

    let format_name = v["format"]["format_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let fmt_duration: f64 = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let empty = vec![];
    let streams = v["streams"].as_array().unwrap_or(&empty);
    let vstream = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or("Видеопоток не найден")?;
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");

    let width = vstream["width"].as_u64().unwrap_or(0).min(1 << 16) as u32;
    let height = vstream["height"].as_u64().unwrap_or(0).min(1 << 16) as u32;
    if width == 0 || height == 0 {
        return Err("Не удалось определить размеры кадра".into());
    }
    let vcodec = vstream["codec_name"].as_str().unwrap_or("").to_string();
    let pix_fmt = vstream["pix_fmt"].as_str().unwrap_or("");
    let nb_frames: u64 = vstream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut fps = parse_rate(vstream["avg_frame_rate"].as_str().unwrap_or("0/1"));
    if fps <= 0.0 {
        fps = parse_rate(vstream["r_frame_rate"].as_str().unwrap_or("0/1"));
    }

    let is_image_fmt = IMAGE_FORMATS.iter().any(|f| format_name.contains(f));
    let mut kind = if is_image_fmt && nb_frames <= 1 {
        "image"
    } else if format_name == "gif" {
        if fmt_duration > 0.09 || nb_frames > 1 {
            "anim"
        } else {
            "image"
        }
    } else if format_name == "webp_anim" || format_name.contains("apng") || is_image_fmt {
        "anim"
    } else {
        "video"
    };

    // Контейнеры без длительности (например webp_anim): считаем кадры сами,
    // иначе обрезка по умолчанию выродится в 0.05 с.
    let mut duration = if kind == "image" { 0.0 } else { fmt_duration };
    if kind != "image" && duration <= 0.0 {
        let packets = count_packets(path).unwrap_or(0);
        if packets <= 1 {
            kind = "image";
        } else {
            let f = if fps > 0.0 { fps } else { 10.0 };
            duration = packets as f64 / f;
        }
    }

    // Прозрачный VP9/VP8 в webm ffprobe показывает как yuv420p + тег alpha_mode=1
    let alpha_mode_tag = vstream["tags"]["alpha_mode"].as_str() == Some("1");
    let has_alpha = ALPHA_PIX_FMTS.contains(&pix_fmt) || alpha_mode_tag;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let browser_playable = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => true,
        // AV1 поддержан WebView не везде (особенно Android) — идёт через прокси
        "mp4" | "m4v" => vcodec == "h264",
        "webm" => matches!(vcodec.as_str(), "vp8" | "vp9"),
        _ => false,
    };

    Ok(MediaInfo {
        kind: kind.to_string(),
        width,
        height,
        duration,
        fps,
        size_bytes,
        vcodec,
        format_name,
        has_alpha,
        has_audio,
        browser_playable,
    })
}

/// Число видеопакетов — используется, когда контейнер не сообщает длительность.
fn count_packets(path: &Path) -> Result<u64, String> {
    let out = cmd("ffprobe")
        .args([
            "-v",
            "error",
            "-count_packets",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| "count_packets: нет данных".to_string())
}

/// Длительность готового файла (проверка лимита 3 с).
pub fn out_duration(path: &Path) -> Result<f64, String> {
    let out = cmd("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| "Не удалось определить длительность".to_string())
}
