"use strict";
const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const MAX_KB = 256;
const MAX_DUR = 3.0;
const MEDIA_EXT = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "apng", "mp4", "m4v", "mov", "mkv", "webm", "avi"];

// на Android нет буфера файлов и проводника — соответствующие кнопки скрываются
const IS_ANDROID = /android/i.test(navigator.userAgent);

const rowsEl = document.getElementById("rows");
const emptyHint = document.getElementById("empty-hint");
const tpl = document.getElementById("row-template");

/** @type {Map<string, object>} */
const rows = new Map();
let seq = 0;
let outDir = null; // null = рядом с исходником
let settings = {};

/* ---------------- утилиты ---------------- */

function fmtSize(bytes) {
  if (bytes == null) return "";
  if (bytes < 1024) return `${bytes} Б`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} КБ`;
  return `${(kb / 1024).toFixed(1)} МБ`;
}
function fmtDur(s) {
  return `${s.toFixed(1)} с`;
}
function baseName(p) {
  return p.split(/[\\/]/).pop();
}
function extOf(p) {
  const m = baseName(p).match(/\.([a-z0-9]+)$/i);
  return m ? m[1].toLowerCase() : "";
}

let toastTimer = null;
function toast(msg, ok = false) {
  const t = document.getElementById("toast");
  t.textContent = String(msg);
  t.classList.toggle("ok", ok);
  t.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add("hidden"), ok ? 2000 : 4000);
}

/* ---------------- настройки ---------------- */

async function loadSettings() {
  try {
    settings = JSON.parse(await invoke("settings_load"));
  } catch { settings = {}; }
  if (settings.outDir) setOutDir(settings.outDir, false);
}
function saveSettings() {
  settings.outDir = outDir;
  invoke("settings_save", { data: JSON.stringify(settings, null, 2) }).catch(() => {});
}
function setOutDir(dir, save = true) {
  outDir = dir || null;
  const btn = document.getElementById("btn-outdir");
  btn.textContent = outDir ? outDir : "рядом с исходником";
  btn.title = outDir || "Выбрать папку вывода";
  document.getElementById("btn-outdir-reset").classList.toggle("hidden", !outDir);
  if (save) saveSettings();
}

/* ---------------- добавление файлов ---------------- */

async function addFiles(paths) {
  const fresh = paths.filter(
    (p) => MEDIA_EXT.includes(extOf(p)) && ![...rows.values()].some((r) => r.path === p)
  );
  for (const path of fresh) {
    const id = `f${++seq}`;
    const row = {
      id, path,
      name: baseName(path),
      info: null,
      params: { start: 0, end: MAX_DUR, w: 512, h: 512, scaleMode: "stretch", format: "vp9", speed: false },
      status: "idle",
      out: null,
      el: null,
    };
    rows.set(id, row);
    buildRow(row);
    emptyHint.classList.add("hidden");
    probeRow(row);
  }
}

async function probeRow(row) {
  try {
    row.info = await invoke("probe_file", { path: row.path });
  } catch (e) {
    setStatus(row, "error", "Ошибка");
    showErr(row, String(e));
    return;
  }
  const i = row.info;
  row.params.end = i.kind === "image" ? 0 : Math.min(MAX_DUR, i.duration);
  renderMeta(row);
  setupTrim(row);
  setupPreview(row);
}

/* ---------------- построение строки ---------------- */

function q(row, sel) { return row.el.querySelector(sel); }

function buildRow(row) {
  const el = tpl.content.firstElementChild.cloneNode(true);
  row.el = el;
  el.dataset.id = row.id;
  q(row, ".fname").textContent = row.name;
  q(row, ".fname").title = row.path;
  q(row, ".media-box").innerHTML = `<div class="placeholder">Читаю файл…</div>`;

  q(row, ".p-scale").addEventListener("change", (e) => (row.params.scaleMode = e.target.value));
  q(row, ".p-format").addEventListener("change", (e) => (row.params.format = e.target.value));
  q(row, ".p-w").addEventListener("change", (e) => (row.params.w = clampSize(e.target)));
  q(row, ".p-h").addEventListener("change", (e) => (row.params.h = clampSize(e.target)));
  q(row, ".b-convert").addEventListener("click", () => enqueue([row]));
  q(row, ".b-cancel").addEventListener("click", () => cancelRow(row));
  q(row, ".b-folder").addEventListener("click", () =>
    row.out && invoke("reveal", { path: row.out.path }).catch(toast)
  );
  q(row, ".b-copy").addEventListener("click", () =>
    row.out &&
    invoke("clipboard_copy_file", { path: row.out.path })
      .then(() => toast("Файл скопирован в буфер обмена", true))
      .catch(toast)
  );
  q(row, ".b-remove").addEventListener("click", () => removeRow(row));

  rowsEl.appendChild(el);
}

function clampSize(input) {
  let v = parseInt(input.value, 10) || 512;
  v = Math.max(64, Math.min(512, Math.round(v / 2) * 2));
  input.value = v;
  return v;
}

function removeRow(row) {
  if (row.status === "working") cancelRow(row);
  if (row.previewUrl) URL.revokeObjectURL(row.previewUrl);
  if (row.outUrl) URL.revokeObjectURL(row.outUrl);
  row.el.remove();
  rows.delete(row.id);
  if (rows.size === 0) emptyHint.classList.remove("hidden");
}

function renderMeta(row) {
  const i = row.info;
  const parts = [`${i.width}×${i.height}`];
  if (i.kind !== "image") parts.push(fmtDur(i.duration), `${Math.round(i.fps)} fps`);
  parts.push(fmtSize(i.size_bytes));
  q(row, ".fmeta").textContent = parts.join(" · ");
  updateSizes(row);
}

/* ---------------- предпросмотр входа ---------------- */

/// Видео в <video> загружается целиком через Blob: потоковая отдача
/// asset-протокола (Range-запросы) нестабильна в WebView2 и Android WebView —
/// зависания со спиннером, обрыв при прокрутке, «то играет, то нет».
async function blobSrc(path) {
  const resp = await fetch(convertFileSrc(path));
  if (!resp.ok) throw new Error(`Не удалось прочитать файл (${resp.status})`);
  return URL.createObjectURL(await resp.blob());
}

// крупный исходник целиком в память не тянем — для него делается прокси
const BLOB_LIMIT = 64 * 1024 * 1024;

/// <video> с бейджем «текущая / общая, с»: нативные контролы WebView2
/// прячут таймер на узком элементе.
function videoWithBadge(src) {
  const wrap = document.createElement("div");
  wrap.className = "video-wrap";
  const v = document.createElement("video");
  v.controls = true;
  v.muted = true;
  v.loop = true;
  v.playsInline = true;
  v.preload = "auto";
  v.src = src;
  const b = document.createElement("div");
  b.className = "time-badge hidden";
  const upd = () => {
    // для одно-кадрового стикера из фото таймер бессмыслен
    if (!isFinite(v.duration) || v.duration < 0.1) return;
    b.textContent = `${v.currentTime.toFixed(1)} / ${v.duration.toFixed(1)} с`;
    b.classList.remove("hidden");
  };
  v.addEventListener("timeupdate", upd);
  v.addEventListener("loadedmetadata", upd);
  wrap.append(v, b);
  return wrap;
}

async function setupPreview(row) {
  const box = q(row, ".media-box");
  const i = row.info;
  if (i.kind === "image") {
    box.innerHTML = "";
    const img = document.createElement("img");
    img.src = convertFileSrc(row.path);
    box.appendChild(img);
    return;
  }
  if (["gif", "webp"].includes(extOf(row.path)) && i.browser_playable) {
    box.innerHTML = "";
    const img = document.createElement("img");
    img.src = convertFileSrc(row.path);
    box.appendChild(img);
    return;
  }
  box.innerHTML = `<div class="placeholder">Готовлю предпросмотр…</div>`;
  try {
    const path =
      i.browser_playable && i.size_bytes <= BLOB_LIMIT
        ? row.path
        : await invoke("make_preview", { input: row.path });
    if (row.previewUrl) URL.revokeObjectURL(row.previewUrl);
    row.previewUrl = await blobSrc(path);
  } catch (e) {
    box.innerHTML = `<div class="placeholder">Предпросмотр недоступен</div>`;
    return;
  }
  box.innerHTML = "";
  box.appendChild(videoWithBadge(row.previewUrl));
}

/* ---------------- обрезка длительности ---------------- */

function setupTrim(row) {
  const i = row.info;
  if (i.kind === "image") {
    q(row, ".trim-block").classList.add("hidden");
    q(row, ".static-note").classList.remove("hidden");
    return;
  }
  const rs = q(row, ".r-start");
  const re = q(row, ".r-end");
  const dur = Math.max(0.1, i.duration);
  rs.max = re.max = dur.toFixed(2);
  rs.value = row.params.start;
  re.value = row.params.end;

  const apply = (moved) => {
    let s = parseFloat(rs.value);
    let e = parseFloat(re.value);
    // при включённой «Скорости» участок любой длины ретаймится в 3 с
    const span = row.params.speed ? dur : MAX_DUR;
    if (e - s < 0.1) {
      if (moved === "start") s = Math.max(0, e - 0.1);
      else e = Math.min(dur, s + 0.1);
    }
    if (e - s > span) {
      if (moved === "start") e = s + span;
      else s = e - span;
    }
    rs.value = s;
    re.value = e;
    row.params.start = s;
    row.params.end = e;
    const len = e - s;
    q(row, ".t-start").textContent = s.toFixed(2);
    q(row, ".t-end").textContent = e.toFixed(2);
    q(row, ".t-len").textContent =
      row.params.speed && Math.abs(len - MAX_DUR) > 0.01
        ? `${len.toFixed(2)}→3.00 (×${(len / MAX_DUR).toFixed(2)})`
        : len.toFixed(2);
    const fill = q(row, ".trim-fill");
    fill.style.left = `${(s / dur) * 100}%`;
    fill.style.width = `${(len / dur) * 100}%`;
  };

  q(row, ".p-speed").addEventListener("change", (e) => {
    row.params.speed = e.target.checked;
    apply("end"); // пересчёт ограничений: при выключении участок ужмётся до 3 с
  });
  rs.addEventListener("input", () => apply("start"));
  re.addEventListener("input", () => apply("end"));

  // перетаскивание всего окна обрезки за среднюю часть между ползунками
  const fill = q(row, ".trim-fill");
  fill.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    const rect = q(row, ".trim").getBoundingClientRect();
    const s0 = parseFloat(rs.value);
    const len = parseFloat(re.value) - s0;
    const x0 = ev.clientX;
    fill.setPointerCapture(ev.pointerId);
    const onMove = (m) => {
      const ds = ((m.clientX - x0) / rect.width) * dur;
      const s = Math.min(Math.max(0, s0 + ds), dur - len);
      rs.value = s;
      re.value = s + len;
      apply("move");
    };
    const onUp = () => {
      fill.removeEventListener("pointermove", onMove);
      fill.removeEventListener("pointerup", onUp);
      fill.removeEventListener("pointercancel", onUp);
    };
    fill.addEventListener("pointermove", onMove);
    fill.addEventListener("pointerup", onUp);
    fill.addEventListener("pointercancel", onUp);
  });

  apply("end");
}

/* ---------------- статусы и вывод ---------------- */

function setStatus(row, status, label) {
  row.status = status;
  const b = q(row, ".badge");
  b.className = `badge ${status}`;
  b.textContent = label;
  q(row, ".b-cancel").classList.toggle("hidden", status !== "working" && status !== "queued");
  q(row, ".b-convert").classList.toggle("hidden", status === "working" || status === "queued");
  q(row, ".progress").classList.toggle("hidden", status !== "working");
  if (status !== "error") showErr(row, null);
}

function showErr(row, msg) {
  const e = q(row, ".err");
  e.classList.toggle("hidden", !msg);
  e.textContent = msg || "";
}

function updateSizes(row) {
  const el = q(row, ".sizes");
  const inS = row.info ? fmtSize(row.info.size_bytes) : "";
  if (row.out) {
    const cls = row.out.fits ? "ok" : "bad";
    el.innerHTML = `${inS} → <b class="${cls}">${fmtSize(row.out.size)}</b>`;
  } else {
    el.textContent = inS;
  }
}

async function showResult(row) {
  if (IS_ANDROID) {
    q(row, ".b-folder").classList.add("android-hidden");
    q(row, ".b-copy").classList.add("android-hidden");
  }
  const box = q(row, ".out-media");
  box.classList.remove("hidden");
  box.innerHTML = "";
  let src;
  try {
    // выход ≤256 КБ — Blob вместо потоковой отдачи asset-протокола
    if (row.outUrl) URL.revokeObjectURL(row.outUrl);
    row.outUrl = await blobSrc(row.out.path);
    src = row.outUrl;
  } catch (e) {
    src = convertFileSrc(row.out.path) + `?v=${Date.now()}`;
  }
  box.appendChild(videoWithBadge(src));
  q(row, ".b-folder").classList.remove("hidden");
  q(row, ".b-copy").classList.remove("hidden");
  q(row, ".b-convert").textContent = "Повторить";
}

/* ---------------- конвертация и очередь ---------------- */

const queue = [];
let active = 0;
const CONCURRENCY = Math.max(1, Math.floor((navigator.hardwareConcurrency || 4) / 2));

function enqueue(list) {
  for (const row of list) {
    if (!row.info || row.status === "working" || row.status === "queued") continue;
    setStatus(row, "queued", "В очереди");
    queue.push(row);
  }
  pump();
}

function pump() {
  while (active < CONCURRENCY && queue.length) {
    const row = queue.shift();
    if (!rows.has(row.id) || row.status !== "queued") continue;
    active++;
    runConvert(row).finally(() => {
      active--;
      pump();
    });
  }
}

async function runConvert(row) {
  setStatus(row, "working", "Кодирование…");
  const p = row.params;
  try {
    const res = await invoke("convert", {
      id: row.id,
      params: {
        input: row.path,
        kind: row.info.kind,
        out_dir: outDir,
        out_path: row.out ? row.out.path : null,
        trim_start: p.start,
        trim_end: p.end,
        width: p.w,
        height: p.h,
        scale_mode: p.scaleMode,
        speed: p.speed,
        format: p.format,
        fps_limit: 30,
        input_fps: row.info.fps,
        has_alpha: row.info.has_alpha,
        max_kb: MAX_KB,
      },
    });
    row.out = { path: res.out_path, size: res.out_size, fits: res.fits };
    if (res.fits) {
      const extra = res.attempts > 1 ? ` · ${res.attempts} поп.` : "";
      setStatus(row, "done", `Готово${extra}`);
    } else {
      setStatus(row, "toobig", `Больше ${MAX_KB} КБ`);
      showErr(row, `Не удалось уместить в ${MAX_KB} КБ за ${res.attempts} попыток. Уменьшите длительность или размер кадра.`);
    }
    updateSizes(row);
    showResult(row);
    // невалидный результат (больше лимита) в галерею не публикуется
    if (IS_ANDROID) {
      if (row.out.fits) saveToGallery(row);
      else toast("Больше 256 КБ — в галерею не сохранено. Уменьшите длительность");
    }
  } catch (e) {
    const msg = String(e);
    if (msg.includes("Отменено")) {
      setStatus(row, "idle", "Ожидает");
    } else {
      setStatus(row, "error", "Ошибка");
      showErr(row, msg);
    }
  }
}

/// Android: выбор папки вывода недоступен, поэтому готовый файл всегда
/// автоматически копируется в общедоступную галерею (Movies/Sticker-Nah).
async function saveToGallery(row) {
  if (!row.out) return;
  try {
    const filename = baseName(row.out.path);
    await invoke("android_save_to_gallery", { path: row.out.path, filename });
    toast("Сохранено в галерею: Sticker-Nah", true);
  } catch (e) {
    toast(`Не удалось сохранить в галерею: ${e}`);
  }
}

function cancelRow(row) {
  invoke("cancel", { id: row.id }).catch(() => {});
  const qi = queue.indexOf(row);
  if (qi >= 0) queue.splice(qi, 1);
  if (row.status === "queued") setStatus(row, "idle", "Ожидает");
}

listen("convert-progress", (ev) => {
  const { id, attempt, pass, pct } = ev.payload;
  const row = rows.get(id);
  if (!row || row.status !== "working") return;
  const overall = pass === 1 ? pct * 45 : 45 + pct * 55;
  q(row, ".bar").style.width = `${overall.toFixed(0)}%`;
  q(row, ".ptext").textContent =
    row.info.kind === "image"
      ? `Попытка ${attempt}`
      : `Попытка ${attempt} · проход ${pass}/2 · ${Math.round(overall)}%`;
});

/* ---------------- панель инструментов ---------------- */

document.getElementById("btn-add").addEventListener("click", async () => {
  try {
    if (IS_ANDROID) {
      // SAF-пикер + копирование во временный файл целиком на стороне Rust —
      // это единственный способ получить путь, который умеет открыть ffmpeg
      const paths = await invoke("android_pick_files");
      if (paths && paths.length) addFiles(paths);
      return;
    }
    const sel = await dialog.open({
      multiple: true,
      filters: [{ name: "Медиафайлы", extensions: MEDIA_EXT }],
    });
    if (sel) addFiles(Array.isArray(sel) ? sel : [sel]);
  } catch (e) {
    toast(e);
  }
});

async function pasteFromClipboard() {
  try {
    const paths = await invoke("clipboard_paste");
    addFiles(paths);
  } catch (e) {
    toast(e);
  }
}
document.getElementById("btn-paste").addEventListener("click", pasteFromClipboard);
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.code === "KeyV") pasteFromClipboard();
});

document.getElementById("btn-convert-all").addEventListener("click", () => {
  enqueue([...rows.values()]);
});

document.getElementById("btn-outdir").addEventListener("click", async () => {
  try {
    const sel = await dialog.open({ directory: true });
    if (sel) setOutDir(sel);
  } catch (e) {
    toast(e);
  }
});
document.getElementById("btn-outdir-reset").addEventListener("click", () => setOutDir(null));

document.getElementById("btn-clear").addEventListener("click", () => {
  for (const row of [...rows.values()]) removeRow(row);
});

/* ---------------- drag & drop ---------------- */

listen("tauri://drag-enter", () => rowsEl.classList.add("dragging"));
listen("tauri://drag-leave", () => rowsEl.classList.remove("dragging"));
listen("tauri://drag-drop", (ev) => {
  rowsEl.classList.remove("dragging");
  if (ev.payload && ev.payload.paths) addFiles(ev.payload.paths);
});

if (IS_ANDROID) {
  document.getElementById("btn-paste").classList.add("android-hidden");
  document.getElementById("outdir-box").classList.add("android-hidden");
  emptyHint.innerHTML = `
    <div class="big">Нажмите «+ Файлы»</div>
    <div>чтобы добавить фото, GIF или видео</div>
    <div class="muted small">Фото · GIF · видео → WebM VP9 · 512×512 · ≤ 256 КБ · ≤ 3 с</div>`;
}

loadSettings();
