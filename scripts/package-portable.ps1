# Builds portable Sticker Nah folder: exe + ffmpeg into release\StickerNah\
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

Set-Location "$root\src-tauri"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = "$root\release\StickerNah"
New-Item -ItemType Directory -Force "$out\bin" | Out-Null
Copy-Item "$root\src-tauri\target\release\sticker-nah.exe" "$out\Sticker Nah.exe" -Force
Copy-Item "$root\src-tauri\bin\win\ffmpeg.exe", "$root\src-tauri\bin\win\ffprobe.exe" "$out\bin\" -Force

$size = [math]::Round((Get-ChildItem $out -Recurse | Measure-Object Length -Sum).Sum / 1MB, 1)
Write-Host "Done: $out ($size MB)"
