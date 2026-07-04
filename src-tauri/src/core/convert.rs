use super::ffmpeg::run_ffmpeg;
use super::{probe, temp_dir, unique_id};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_DURATION: f64 = 3.0;

#[derive(Deserialize, Clone, Debug)]
pub struct ConvertParams {
    pub input: String,
    pub kind: String, // "image" | "anim" | "video"
    pub out_dir: Option<String>,
    /// Путь выхода с прошлой попытки — перезаписываем его же при повторе
    pub out_path: Option<String>,
    pub trim_start: f64,
    pub trim_end: f64,
    pub width: u32,
    pub height: u32,
    pub scale_mode: String, // "stretch" | "cover"
    pub fps_limit: f64,     // 30
    pub input_fps: f64,
    pub has_alpha: bool,
    pub max_kb: u64, // 256
}

#[derive(Serialize, Clone, Debug)]
pub struct ConvertResult {
    pub out_path: String,
    pub out_size: u64,
    /// true — уложились в лимит размера
    pub fits: bool,
    pub duration: f64,
    pub attempts: u32,
    pub bitrate_kbps: u32,
    pub fps: f64,
}

pub struct Progress<'a> {
    /// (attempt, pass, pct_пасса 0..1)
    pub emit: &'a mut dyn FnMut(u32, u32, f64),
    pub register_pid: &'a dyn Fn(u32),
    pub is_cancelled: &'a dyn Fn() -> bool,
}

fn vf_chain(p: &ConvertParams, fps: Option<f64>) -> String {
    let (w, h) = (p.width, p.height);
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = fps {
        parts.push(format!("fps={f:.4}"));
    }
    match p.scale_mode.as_str() {
        "cover" => parts.push(format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase:flags=lanczos,crop={w}:{h}"
        )),
        _ => parts.push(format!("scale={w}:{h}:flags=lanczos")),
    }
    parts.push(format!(
        "format={}",
        if p.has_alpha { "yuva420p" } else { "yuv420p" }
    ));
    parts.join(",")
}

/// Свободное имя выхода: `{имя}_sticker.webm`, при коллизии `{имя}_sticker (n).webm`.
fn pick_out_path(p: &ConvertParams) -> Result<PathBuf, String> {
    if let Some(op) = &p.out_path {
        return Ok(PathBuf::from(op));
    }
    let input = Path::new(&p.input);
    let dir = match &p.out_dir {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => input
            .parent()
            .map(|d| d.to_path_buf())
            .ok_or("Нет папки исходника")?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("Папка вывода: {e}"))?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sticker");
    let mut candidate = dir.join(format!("{stem}_sticker.webm"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}_sticker ({n}).webm"));
        n += 1;
    }
    Ok(candidate)
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX)
}

pub fn convert(p: &ConvertParams, prog: &mut Progress) -> Result<ConvertResult, String> {
    let out = pick_out_path(p)?;
    if p.kind == "image" {
        convert_image(p, &out, prog)
    } else {
        convert_video(p, &out, prog)
    }
}

/// Статика: 1 кадр, CRF-режим, лестница качества при превышении размера.
fn convert_image(
    p: &ConvertParams,
    out: &Path,
    prog: &mut Progress,
) -> Result<ConvertResult, String> {
    let max_bytes = p.max_kb * 1024;
    let vf = vf_chain(p, None);
    let crf_ladder = [30u32, 40, 50, 63];
    let mut attempts = 0;
    for crf in crf_ladder {
        if (prog.is_cancelled)() {
            return Err("Отменено".into());
        }
        attempts += 1;
        (prog.emit)(attempts, 1, 0.0);
        let args: Vec<String> = vec![
            "-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into(),
            "-progress".into(), "pipe:1".into(), "-nostats".into(),
            "-i".into(), p.input.clone(),
            "-map".into(), "0:v:0".into(),
            "-frames:v".into(), "1".into(),
            "-vf".into(), vf.clone(),
            "-c:v".into(), "libvpx-vp9".into(),
            "-crf".into(), crf.to_string(),
            "-b:v".into(), "0".into(),
            "-cpu-used".into(), "1".into(),
            "-row-mt".into(), "1".into(),
            "-an".into(), "-sn".into(),
            "-map_metadata".into(), "-1".into(),
            out.to_string_lossy().into_owned(),
        ];
        run_ffmpeg(&args, 0.0, |_| {}, |pid| (prog.register_pid)(pid))?;
        (prog.emit)(attempts, 1, 1.0);
        let size = file_size(out);
        if size <= max_bytes {
            return Ok(ConvertResult {
                out_path: out.to_string_lossy().into_owned(),
                out_size: size,
                fits: true,
                duration: 0.0,
                attempts,
                bitrate_kbps: 0,
                fps: 0.0,
            });
        }
    }
    Ok(ConvertResult {
        out_path: out.to_string_lossy().into_owned(),
        out_size: file_size(out),
        fits: false,
        duration: 0.0,
        attempts,
        bitrate_kbps: 0,
        fps: 0.0,
    })
}

fn encode_args(
    p: &ConvertParams,
    dur: f64,
    bitrate: u32,
    vf: &str,
    pass_n: u32,
    passlog: &Path,
    out: &Path,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into(),
        "-progress".into(), "pipe:1".into(), "-nostats".into(),
    ];
    if p.trim_start > 0.0 {
        a.extend(["-ss".into(), format!("{:.3}", p.trim_start)]);
    }
    a.extend(["-t".into(), format!("{dur:.3}")]);
    a.extend(["-i".into(), p.input.clone()]);
    a.extend(["-map".into(), "0:v:0".into()]);
    a.extend(["-vf".into(), vf.to_string()]);
    a.extend([
        "-c:v".into(), "libvpx-vp9".into(),
        "-b:v".into(), format!("{bitrate}k"),
        "-minrate".into(), format!("{}k", bitrate / 2),
        "-maxrate".into(), format!("{}k", bitrate * 29 / 20),
        "-deadline".into(), "good".into(),
        "-cpu-used".into(), if pass_n == 1 { "4".into() } else { "1".to_string() },
        "-row-mt".into(), "1".into(),
        "-g".into(), "120".into(),
    ]);
    if p.has_alpha {
        // альфа-канал в libvpx несовместим с alt-ref кадрами
        a.extend(["-auto-alt-ref".into(), "0".into()]);
    } else {
        a.extend(["-auto-alt-ref".into(), "1".into(), "-lag-in-frames".into(), "25".into()]);
    }
    a.extend(["-an".into(), "-sn".into(), "-map_metadata".into(), "-1".into()]);
    a.extend([
        "-pass".into(), pass_n.to_string(),
        "-passlogfile".into(), passlog.to_string_lossy().into_owned(),
    ]);
    if pass_n == 1 {
        a.extend(["-f".into(), "null".into(), "NUL".into()]);
    } else {
        a.push(out.to_string_lossy().into_owned());
    }
    a
}

/// Видео/анимация: двухпроходный VP9 под целевой битрейт + итеративное снижение.
fn convert_video(
    p: &ConvertParams,
    out: &Path,
    prog: &mut Progress,
) -> Result<ConvertResult, String> {
    let max_bytes = p.max_kb * 1024;
    let target_bytes = max_bytes.saturating_sub(2048) as f64; // запас на контейнер
    let mut dur = (p.trim_end - p.trim_start).clamp(0.05, MAX_DURATION);

    let src_fps = if p.input_fps > 0.0 { p.input_fps } else { p.fps_limit };
    let base_fps = src_fps.min(p.fps_limit);
    let fps_ladder: Vec<f64> = [base_fps, 24.0, 20.0, 15.0]
        .into_iter()
        .filter(|f| *f <= base_fps + 0.001)
        .collect();
    let init_bitrate =
        |d: f64| ((target_bytes * 8.0 / d / 1000.0) * 0.92).clamp(40.0, 4000.0) as u32;

    let passlog = temp_dir().join(format!("pass_{}", unique_id()));
    let mut fps_idx = 0usize;
    let mut bitrate = init_bitrate(dur);
    let mut attempts = 0u32;
    let mut duration_retry_done = false;

    let result = loop {
        if (prog.is_cancelled)() {
            return Err("Отменено".into());
        }
        attempts += 1;
        let fps = fps_ladder[fps_idx];
        let vf = vf_chain(p, Some(fps));

        for pass_n in [1u32, 2] {
            if (prog.is_cancelled)() {
                return Err("Отменено".into());
            }
            let args = encode_args(p, dur, bitrate, &vf, pass_n, &passlog, out);
            let a = attempts;
            run_ffmpeg(
                &args,
                dur,
                |pct| (prog.emit)(a, pass_n, pct),
                |pid| (prog.register_pid)(pid),
            )
            .map_err(|e| {
                if (prog.is_cancelled)() {
                    "Отменено".to_string()
                } else {
                    e
                }
            })?;
        }

        let size = file_size(out);
        if size <= max_bytes {
            // контроль длительности контейнера (округления matroska)
            let real_dur = probe::out_duration(out).unwrap_or(dur);
            if real_dur > MAX_DURATION + 0.005 && !duration_retry_done {
                duration_retry_done = true;
                dur = (dur - (real_dur - MAX_DURATION) - 0.02).max(0.1);
                continue;
            }
            break ConvertResult {
                out_path: out.to_string_lossy().into_owned(),
                out_size: size,
                fits: true,
                duration: real_dur,
                attempts,
                bitrate_kbps: bitrate,
                fps,
            };
        }
        if attempts >= 8 {
            break ConvertResult {
                out_path: out.to_string_lossy().into_owned(),
                out_size: size,
                fits: false,
                duration: dur,
                attempts,
                bitrate_kbps: bitrate,
                fps,
            };
        }
        // промах: пропорциональное снижение битрейта с запасом
        let ratio = target_bytes / size as f64;
        bitrate = ((bitrate as f64) * ratio * 0.93).max(40.0) as u32;
        // качество битрейтом уже не спасти — снижаем fps и пересчитываем
        if bitrate < 150 && fps_idx + 1 < fps_ladder.len() {
            fps_idx += 1;
            bitrate = init_bitrate(dur);
        }
    };

    // подчистить логи двухпроходности
    let _ = std::fs::remove_file(passlog.with_extension("log"));
    let _ = std::fs::remove_file(PathBuf::from(format!(
        "{}-0.log",
        passlog.to_string_lossy()
    )));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn params(name: &str, kind: &str, dur: f64, fps: f64, scale_mode: &str) -> ConvertParams {
        ConvertParams {
            input: fixture(name),
            kind: kind.into(),
            out_dir: Some(std::env::temp_dir().join("sticker-nah-tests").to_string_lossy().into_owned()),
            out_path: None,
            trim_start: 0.0,
            trim_end: dur.min(MAX_DURATION),
            width: 512,
            height: 512,
            scale_mode: scale_mode.into(),
            fps_limit: 30.0,
            input_fps: fps,
            has_alpha: false,
            max_kb: 256,
        }
    }

    fn run(p: &ConvertParams) -> ConvertResult {
        let mut emit = |_: u32, _: u32, _: f64| {};
        let mut prog = Progress {
            emit: &mut emit,
            register_pid: &|_| {},
            is_cancelled: &|| false,
        };
        let r = convert(p, &mut prog).expect("конвертация не должна падать");
        let _ = std::fs::remove_file(&r.out_path);
        r
    }

    fn assert_valid(r: &ConvertResult, is_video: bool) {
        assert!(r.fits, "должно уместиться в 256 КБ, вышло {} Б", r.out_size);
        assert!(r.out_size <= 256 * 1024);
        if is_video {
            assert!(r.duration <= MAX_DURATION + 0.005, "длительность {}", r.duration);
        }
    }

    #[test]
    fn video_1080p_stretch() {
        let r = run(&params("long_1080p.mp4", "video", 10.0, 30.0, "stretch"));
        assert_valid(&r, true);
    }

    #[test]
    fn video_vertical_cover_drops_audio() {
        let p = params("vertical_audio.mp4", "video", 5.0, 25.0, "cover");
        let mut emit = |_: u32, _: u32, _: f64| {};
        let mut prog = Progress {
            emit: &mut emit,
            register_pid: &|_| {},
            is_cancelled: &|| false,
        };
        let r = convert(&p, &mut prog).unwrap();
        assert_valid(&r, true);
        let info = crate::core::probe::probe(Path::new(&r.out_path)).unwrap();
        assert!(!info.has_audio, "аудио должно быть удалено");
        assert_eq!((info.width, info.height), (512, 512));
        assert_eq!(info.vcodec, "vp9");
        let _ = std::fs::remove_file(&r.out_path);
    }

    #[test]
    fn gif_anim() {
        let r = run(&params("anim.gif", "anim", 4.0, 15.0, "stretch"));
        assert_valid(&r, true);
    }

    #[test]
    fn static_image() {
        let mut p = params("photo.png", "image", 0.0, 0.0, "cover");
        p.trim_end = 0.0;
        let r = run(&p);
        assert_valid(&r, false);
    }
}

/// Быстрый низкокачественный прокси для предпросмотра неподдерживаемых браузером форматов.
pub fn make_proxy(input: &str, register_pid: impl FnOnce(u32)) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    let mtime = std::fs::metadata(input)
        .and_then(|m| m.modified())
        .map(|t| format!("{t:?}"))
        .unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (input, mtime).hash(&mut h);
    let out = temp_dir().join(format!("proxy_{:016x}.webm", h.finish()));
    if out.exists() {
        return Ok(out.to_string_lossy().into_owned());
    }
    let args: Vec<String> = vec![
        "-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into(),
        "-progress".into(), "pipe:1".into(), "-nostats".into(),
        "-i".into(), input.to_string(),
        "-map".into(), "0:v:0".into(),
        "-vf".into(), "fps=15,scale=-2:256:flags=bilinear,format=yuv420p".into(),
        "-c:v".into(), "libvpx".into(),
        "-crf".into(), "40".into(),
        "-b:v".into(), "600k".into(),
        "-deadline".into(), "realtime".into(),
        "-cpu-used".into(), "16".into(),
        "-an".into(), "-sn".into(),
        out.to_string_lossy().into_owned(),
    ];
    run_ffmpeg(&args, 0.0, |_| {}, register_pid)?;
    Ok(out.to_string_lossy().into_owned())
}
