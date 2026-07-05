# Builds single-file portable Sticker Nah: release\Sticker Nah.exe
# (ffmpeg/ffprobe embedded, extracted to %LOCALAPPDATA%\StickerNah on first run)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

Set-Location "$root\src-tauri"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force "$root\release" | Out-Null
Copy-Item "$root\src-tauri\target\release\sticker-nah.exe" "$root\release\Sticker Nah.exe" -Force

$size = [math]::Round((Get-Item "$root\release\Sticker Nah.exe").Length / 1MB, 1)
Write-Host "Done: $root\release\Sticker Nah.exe ($size MB)"
