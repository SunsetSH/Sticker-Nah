# Sticker Nah

Конвертер медиафайлов в стикеры: **WebM VP9 · 512×512 · ≤ 256 КБ · ≤ 3 с · без звука**.
Windows + Android (Tauri 2 + Rust + FFmpeg). ТЗ и архитектура: [SPEC.md](SPEC.md).

## Возможности

- Пакетная загрузка: диалог, drag&drop, буфер обмена (Ctrl+V — файлы или картинка).
- Входные форматы: PNG, JPG, WebP, BMP, GIF, APNG, MP4, MOV, MKV, WebM, AVI.
- Обрезка длительности двумя ползунками (по умолчанию 0–3 с, строго ≤ 3 с).
- «Скорость»: ретайминг участка любой длины в ровно 3 с (setpts).
- Масштабирование: растянуть / увеличить и обрезать; размер по умолчанию 512×512.
- Автоподгон под 256 КБ: двухпроходный VP9 с расчётным битрейтом, при промахе —
  итеративное снижение битрейта, затем fps (30→24→20→15), до 8 попыток.
- Предпросмотр входа и результата с таймлайном; размеры «вход → выход».
- Параллельная очередь (ядер/2).
- Windows: один exe-файл (ffmpeg/ffprobe встроены, распаковываются лениво;
  распакованная копия проверяется побайтово при каждом запуске).
- Android: системный выбор файлов (SAF), результат сохраняется в галерею
  (`Movies/Sticker-Nah`) — кнопки «Открыть» и «Поделиться» ведут на сам файл.

## Сборка

**Windows** — Rust (MSVC), Node.js, ffmpeg.exe + ffprobe.exe в `src-tauri/bin/win/`
(собираются `scripts/build-ffmpeg-min-win.sh`, версия FFmpeg закреплена коммитом
+ sha256 в `scripts/ffmpeg-version.sh` — сборка воспроизводима).

```powershell
npm install
npm run dev                    # запуск в dev-режиме
scripts\package-portable.ps1   # портативный release\Sticker Nah.exe
scripts\sign.ps1               # подпись exe (самоподписанный сертификат, см. комментарий в скрипте)
```

**Android** — JDK 21, Android SDK/NDK, ffmpeg для arm64 в `src-tauri/bin/android/`
(собирается `scripts/build-ffmpeg-min-android.sh`).

```powershell
npx tauri android build --apk --target aarch64
```

## Структура

- `src-tauri/src/core/` — ядро: probe, ffmpeg-раннер, алгоритм подгона размера, `platform.rs` — платформенные различия (Windows/Android)
- `src-tauri/src/commands.rs` — команды Tauri (конвертация, буфер, отмена, настройки, Android SAF/галерея)
- `src-tauri/src/lib.rs` / `main.rs` — точка входа (lib обязателен для Android)
- `src/` — UI (vanilla JS, без сборщика), общий для Windows и Android
- `tests/fixtures/` — тестовые медиафайлы
