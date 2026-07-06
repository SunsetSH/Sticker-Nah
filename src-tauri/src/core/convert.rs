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
    /// Ретайминг: выбранный участок ускоряется/замедляется ровно до 3 с
    #[serde(default)]
    pub speed: bool,
    #[serde(default = "default_format")]
    pub format: String, // пока только "vp9"
    pub fps_limit: f64,     // 30
    pub input_fps: f64,
    pub has_alpha: bool,
    pub max_kb: u64, // 256
}

fn default_format() -> String {
    "vp9".into()
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
    /// Some(pid) — процесс запущен, None — завершился
    pub set_pid: &'a dyn Fn(Option<u32>),
    pub is_cancelled: &'a dyn Fn() -> bool,
}

fn vf_chain(p: &ConvertParams, fps: Option<f64>, tempo: f64) -> String {
    let (w, h) = (p.width, p.height);
    let mut parts: Vec<String> = Vec::new();
    if (tempo - 1.0).abs() > 0.001 {
        // ретайминг участка: tempo = out_dur / in_dur
        parts.push(format!("setpts=PTS*{tempo:.6}"));
    }
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
/// Имя резервируется атомарно (create_new), чтобы параллельные задания с одинаковым
/// stem не выбрали один путь.
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
    for n in 0..10_000 {
        let candidate = if n == 0 {
            dir.join(format!("{stem}_sticker.webm"))
        } else {
            dir.join(format!("{stem}_sticker ({n}).webm"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Папка вывода: {e}")),
        }
    }
    Err("Не удалось подобрать свободное имя выхода".into())
}

/// Кодирование идёт во временный файл рядом с целевым; на место он попадает
/// атомарным rename только после успеха.
fn part_path(out: &Path) -> PathBuf {
    out.with_extension("part.webm")
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX)
}

/// Запуск ffmpeg с прогрессом, регистрацией/снятием PID и переводом ошибки
/// в «Отменено», если причина — отмена.
fn run_encoder(
    args: &[String],
    dur: f64,
    prog: &mut Progress,
    attempt: u32,
    pass_n: u32,
) -> Result<(), String> {
    let r = run_ffmpeg(
        args,
        dur,
        |pct| (prog.emit)(attempt, pass_n, pct),
        |pid| (prog.set_pid)(Some(pid)),
    );
    (prog.set_pid)(None);
    r.map_err(|e| {
        if (prog.is_cancelled)() {
            "Отменено".to_string()
        } else {
            e
        }
    })
}

/// Серверная проверка параметров: IPC-границе нельзя доверять (XSS в WebView
/// не должен получить произвольную запись файлов или исчерпание ресурсов).
fn validate(p: &ConvertParams) -> Result<(), String> {
    if p.format != "vp9" {
        return Err(format!("Неподдерживаемый выходной формат: {}", p.format));
    }
    if !matches!(p.scale_mode.as_str(), "stretch" | "cover") {
        return Err("Неподдерживаемый режим масштабирования".into());
    }
    if !(16..=1024).contains(&p.width) || !(16..=1024).contains(&p.height) {
        return Err("Недопустимый размер выхода".into());
    }
    for v in [p.trim_start, p.trim_end, p.fps_limit, p.input_fps] {
        if !v.is_finite() || v < 0.0 || v > 100_000.0 {
            return Err("Недопустимые параметры времени/частоты".into());
        }
    }
    if p.trim_end < p.trim_start {
        return Err("Конец обрезки раньше начала".into());
    }
    if !(16..=4096).contains(&p.max_kb) {
        return Err("Недопустимый лимит размера".into());
    }
    // перезапись разрешена только для результата прежней конвертации
    if let Some(op) = &p.out_path {
        if !op.ends_with(".webm") {
            return Err("Недопустимый путь выхода".into());
        }
    }
    Ok(())
}

pub fn convert(p: &ConvertParams, prog: &mut Progress) -> Result<ConvertResult, String> {
    validate(p)?;
    let out = pick_out_path(p)?;
    let part = part_path(&out);
    let res = if p.kind == "image" {
        convert_image(p, &part, prog)
    } else {
        convert_video(p, &part, prog)
    };
    match res {
        Ok(mut r) => {
            std::fs::rename(&part, &out)
                .map_err(|e| format!("Не удалось сохранить результат: {e}"))?;
            r.out_path = out.to_string_lossy().into_owned();
            Ok(r)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            // убрать зарезервированный пустой файл, чтобы не оставлять мусор
            if p.out_path.is_none()
                && std::fs::metadata(&out)
                    .map(|m| m.len() == 0)
                    .unwrap_or(false)
            {
                let _ = std::fs::remove_file(&out);
            }
            Err(e)
        }
    }
}

/// Статика: 1 кадр, CRF-режим, лестница качества при превышении размера.
fn convert_image(
    p: &ConvertParams,
    part: &Path,
    prog: &mut Progress,
) -> Result<ConvertResult, String> {
    let max_bytes = p.max_kb * 1024;
    let vf = vf_chain(p, None, 1.0);
    let crf_ladder = [30u32, 40, 50, 63];
    let mut attempts = 0;
    let mut size = u64::MAX;
    for crf in crf_ladder {
        if (prog.is_cancelled)() {
            return Err("Отменено".into());
        }
        attempts += 1;
        (prog.emit)(attempts, 1, 0.0);
        let args: Vec<String> = vec![
            "-y".into(),
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-progress".into(),
            "pipe:1".into(),
            "-nostats".into(),
            "-i".into(),
            p.input.clone(),
            "-map".into(),
            "0:v:0".into(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            vf.clone(),
            "-aspect".into(),
            format!("{}:{}", p.width, p.height),
            "-c:v".into(),
            "libvpx-vp9".into(),
            "-crf".into(),
            crf.to_string(),
            "-b:v".into(),
            "0".into(),
            "-cpu-used".into(),
            "1".into(),
            "-row-mt".into(),
            "1".into(),
            "-an".into(),
            "-sn".into(),
            "-map_metadata".into(),
            "-1".into(),
            "-f".into(),
            "webm".into(),
            part.to_string_lossy().into_owned(),
        ];
        run_encoder(&args, 0.0, prog, attempts, 1)?;
        (prog.emit)(attempts, 1, 1.0);
        size = file_size(part);
        if size <= max_bytes {
            break;
        }
    }
    Ok(ConvertResult {
        out_path: part.to_string_lossy().into_owned(),
        out_size: size,
        fits: size <= max_bytes,
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
    part: &Path,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];
    if p.trim_start > 0.0 {
        a.extend(["-ss".into(), format!("{:.3}", p.trim_start)]);
    }
    a.extend(["-t".into(), format!("{dur:.3}")]);
    a.extend(["-i".into(), p.input.clone()]);
    a.extend(["-map".into(), "0:v:0".into()]);
    a.extend(["-vf".into(), vf.to_string()]);
    // scale компенсирует растяжение через SAR и плееры показали бы исходные
    // пропорции; DAR=w:h принуждает SAR к 1:1 — показ равен кодированному кадру
    a.extend(["-aspect".into(), format!("{}:{}", p.width, p.height)]);
    a.extend([
        "-c:v".into(),
        "libvpx-vp9".into(),
        "-b:v".into(),
        format!("{bitrate}k"),
        "-minrate".into(),
        format!("{}k", bitrate / 2),
        "-maxrate".into(),
        format!("{}k", bitrate * 29 / 20),
        "-deadline".into(),
        "good".into(),
        "-cpu-used".into(),
        if pass_n == 1 {
            "4".into()
        } else {
            "1".to_string()
        },
        "-row-mt".into(),
        "1".into(),
        "-g".into(),
        "120".into(),
    ]);
    if p.has_alpha {
        // альфа-канал в libvpx несовместим с alt-ref кадрами
        a.extend(["-auto-alt-ref".into(), "0".into()]);
    } else {
        a.extend([
            "-auto-alt-ref".into(),
            "1".into(),
            "-lag-in-frames".into(),
            "25".into(),
        ]);
    }
    a.extend([
        "-an".into(),
        "-sn".into(),
        "-map_metadata".into(),
        "-1".into(),
    ]);
    a.extend([
        "-pass".into(),
        pass_n.to_string(),
        "-passlogfile".into(),
        passlog.to_string_lossy().into_owned(),
    ]);
    if pass_n == 1 {
        a.extend([
            "-f".into(),
            "null".into(),
            super::platform::null_sink().into(),
        ]);
    } else {
        a.extend(["-f".into(), "webm".into()]);
        a.push(part.to_string_lossy().into_owned());
    }
    a
}

/// Видео/анимация: двухпроходный VP9 под целевой битрейт + итеративное снижение.
fn convert_video(
    p: &ConvertParams,
    part: &Path,
    prog: &mut Progress,
) -> Result<ConvertResult, String> {
    let passlog = temp_dir().join(format!("pass_{}", unique_id()));
    let result = convert_video_inner(p, part, prog, &passlog);
    // логи двухпроходности чистятся на любом исходе, включая ошибку и отмену
    let _ = std::fs::remove_file(passlog.with_extension("log"));
    let _ = std::fs::remove_file(PathBuf::from(format!(
        "{}-0.log",
        passlog.to_string_lossy()
    )));
    result
}

fn convert_video_inner(
    p: &ConvertParams,
    part: &Path,
    prog: &mut Progress,
    passlog: &Path,
) -> Result<ConvertResult, String> {
    let max_bytes = p.max_kb * 1024;
    let target_bytes = max_bytes.saturating_sub(2048) as f64; // запас на контейнер
    // in_dur — сколько читаем из исходника; out_dur — длительность результата.
    // При включённой «Скорости» участок любой длины ретаймится ровно в 3 с.
    let seg = (p.trim_end - p.trim_start).max(0.05);
    let (mut in_dur, mut out_dur) = if p.speed {
        (seg, MAX_DURATION)
    } else {
        let d = seg.min(MAX_DURATION);
        (d, d)
    };

    let src_fps = if p.input_fps > 0.0 {
        p.input_fps
    } else {
        p.fps_limit
    };
    let base_fps = src_fps.min(p.fps_limit);
    let fps_ladder: Vec<f64> = [base_fps, 24.0, 20.0, 15.0]
        .into_iter()
        .filter(|f| *f <= base_fps + 0.001)
        .collect();
    let init_bitrate =
        |d: f64| ((target_bytes * 8.0 / d / 1000.0) * 0.92).clamp(40.0, 4000.0) as u32;

    let mut fps_idx = 0usize;
    let mut bitrate = init_bitrate(out_dur);
    let mut attempts = 0u32;
    let mut duration_retry_done = false;

    loop {
        if (prog.is_cancelled)() {
            return Err("Отменено".into());
        }
        attempts += 1;
        let fps = fps_ladder[fps_idx];
        let tempo = out_dur / in_dur;
        let vf = vf_chain(p, Some(fps), tempo);

        for pass_n in [1u32, 2] {
            if (prog.is_cancelled)() {
                return Err("Отменено".into());
            }
            let args = encode_args(p, in_dur, bitrate, &vf, pass_n, passlog, part);
            run_encoder(&args, out_dur, prog, attempts, pass_n)?;
        }

        let size = file_size(part);
        if size <= max_bytes {
            // контроль длительности контейнера (округления matroska)
            let real_dur = probe::out_duration(part).unwrap_or(out_dur);
            if real_dur > MAX_DURATION + 0.005 && !duration_retry_done {
                duration_retry_done = true;
                in_dur = (in_dur * (MAX_DURATION - 0.02) / real_dur).max(0.1);
                if !p.speed {
                    out_dur = in_dur;
                }
                continue;
            }
            return Ok(ConvertResult {
                out_path: part.to_string_lossy().into_owned(),
                out_size: size,
                fits: true,
                duration: real_dur,
                attempts,
                bitrate_kbps: bitrate,
                fps,
            });
        }
        if attempts >= 8 {
            return Ok(ConvertResult {
                out_path: part.to_string_lossy().into_owned(),
                out_size: size,
                fits: false,
                duration: out_dur,
                attempts,
                bitrate_kbps: bitrate,
                fps,
            });
        }
        // промах: пропорциональное снижение битрейта с запасом
        let ratio = target_bytes / size as f64;
        bitrate = ((bitrate as f64) * ratio * 0.93).max(40.0) as u32;
        // качество битрейтом уже не спасти — снижаем fps и пересчитываем
        if bitrate < 150 && fps_idx + 1 < fps_ladder.len() {
            fps_idx += 1;
            bitrate = init_bitrate(out_dur);
        }
    }
}

/// Быстрый низкокачественный прокси для предпросмотра неподдерживаемых браузером форматов.
/// Пишется во временный файл и попадает в кэш атомарным rename — недописанный
/// после сбоя прокси не считается валидным.
pub fn make_proxy(input: &str, register_pid: impl FnOnce(u32)) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    let md = std::fs::metadata(input).map_err(|e| format!("Файл недоступен: {e}"))?;
    let mtime = md.modified().map(|t| format!("{t:?}")).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (input, mtime, md.len()).hash(&mut h);
    let out = temp_dir().join(format!("proxy_{:016x}.webm", h.finish()));
    if out.exists() && file_size(&out) > 0 {
        return Ok(out.to_string_lossy().into_owned());
    }
    let part = temp_dir().join(format!("proxy_{:016x}_{}.part", h.finish(), unique_id()));
    let args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-i".into(),
        input.to_string(),
        "-map".into(),
        "0:v:0".into(),
        "-vf".into(),
        "fps=15,scale=-2:256:flags=bilinear,format=yuv420p".into(),
        "-c:v".into(),
        "libvpx".into(),
        "-crf".into(),
        "40".into(),
        "-b:v".into(),
        "600k".into(),
        "-deadline".into(),
        "realtime".into(),
        "-cpu-used".into(),
        "16".into(),
        "-an".into(),
        "-sn".into(),
        "-f".into(),
        "webm".into(),
        part.to_string_lossy().into_owned(),
    ];
    let r = run_ffmpeg(&args, 0.0, |_| {}, register_pid);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&part, &out) {
        // параллельный make_proxy того же входа мог опередить — его кэш валиден
        let _ = std::fs::remove_file(&part);
        if !(out.exists() && file_size(&out) > 0) {
            return Err(format!("Кэш предпросмотра: {e}"));
        }
    }
    Ok(out.to_string_lossy().into_owned())
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
            out_dir: Some(
                std::env::temp_dir()
                    .join("sticker-nah-tests")
                    .to_string_lossy()
                    .into_owned(),
            ),
            out_path: None,
            trim_start: 0.0,
            trim_end: dur.min(MAX_DURATION),
            width: 512,
            height: 512,
            scale_mode: scale_mode.into(),
            speed: false,
            format: "vp9".into(),
            fps_limit: 30.0,
            input_fps: fps,
            has_alpha: false,
            max_kb: 256,
        }
    }

    fn run_params(p: &ConvertParams) -> ConvertResult {
        let mut emit = |_: u32, _: u32, _: f64| {};
        let mut prog = Progress {
            emit: &mut emit,
            set_pid: &|_| {},
            is_cancelled: &|| false,
        };
        convert(p, &mut prog).expect("конвертация не должна падать")
    }

    fn run(p: &ConvertParams) -> ConvertResult {
        let r = run_params(p);
        let _ = std::fs::remove_file(&r.out_path);
        r
    }

    fn assert_valid(r: &ConvertResult, is_video: bool) {
        assert!(r.fits, "должно уместиться в 256 КБ, вышло {} Б", r.out_size);
        assert!(r.out_size <= 256 * 1024);
        if is_video {
            assert!(
                r.duration <= MAX_DURATION + 0.005,
                "длительность {}",
                r.duration
            );
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
        let r = run_params(&p);
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

    /// «Скорость»: участок 6 с ускоряется ровно до 3 с; участок 1.5 с замедляется до 3 с.
    #[test]
    fn speed_retiming() {
        // ускорение: 0..6 с -> 3 с
        let mut p = params("long_1080p.mp4", "video", 10.0, 30.0, "stretch");
        p.speed = true;
        p.trim_end = 6.0;
        let r = run(&p);
        assert_valid(&r, true);
        assert!(
            (r.duration - MAX_DURATION).abs() < 0.05,
            "ускорение: длительность должна быть ~3 с, получено {}",
            r.duration
        );
        // замедление: 0..1.5 с -> 3 с
        let mut p2 = params("long_1080p.mp4", "video", 10.0, 30.0, "stretch");
        p2.speed = true;
        p2.trim_end = 1.5;
        let r2 = run(&p2);
        assert_valid(&r2, true);
        assert!(
            (r2.duration - MAX_DURATION).abs() < 0.05,
            "замедление: длительность должна быть ~3 с, получено {}",
            r2.duration
        );
    }

    /// MED-06: анимированный webp определяется как anim с вычисленной длительностью.
    #[test]
    fn animated_webp_probe_and_convert() {
        let path = fixture("anim.webp");
        let info = crate::core::probe::probe(Path::new(&path)).unwrap();
        assert_eq!(info.kind, "anim", "webp_anim должен быть anim");
        assert!(
            (info.duration - 2.0).abs() < 0.3,
            "длительность ~2 с, получено {}",
            info.duration
        );
        assert!(info.has_alpha, "argb-пиксели webp несут альфу");
        let mut p = params("anim.webp", "anim", info.duration, info.fps, "stretch");
        p.has_alpha = info.has_alpha;
        let r = run(&p);
        assert_valid(&r, true);
    }

    /// CRIT-03: параллельные задания с одинаковым stem не должны выбрать один путь.
    #[test]
    fn parallel_same_stem_no_clobber() {
        let out_dir = std::env::temp_dir().join("sticker-nah-tests-race");
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).unwrap();
        let mk = || {
            let mut p = params("photo.png", "image", 0.0, 0.0, "stretch");
            p.out_dir = Some(out_dir.to_string_lossy().into_owned());
            p.trim_end = 0.0;
            p
        };
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = mk();
                std::thread::spawn(move || run_params(&p).out_path)
            })
            .collect();
        let mut paths: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), 4, "пути должны быть уникальны: {paths:?}");
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    /// Неподдерживаемый формат отклоняется до кодирования.
    #[test]
    fn unknown_format_rejected() {
        let mut p = params("photo.png", "image", 0.0, 0.0, "stretch");
        p.format = "gif".into();
        let mut emit = |_: u32, _: u32, _: f64| {};
        let mut prog = Progress {
            emit: &mut emit,
            set_pid: &|_| {},
            is_cancelled: &|| false,
        };
        assert!(convert(&p, &mut prog).is_err());
    }
}
