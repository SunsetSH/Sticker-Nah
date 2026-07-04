# Generates synthetic test media into tests\fixtures (not committed to git)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$ff = "$root\src-tauri\bin\win\ffmpeg.exe"
$d = "$root\tests\fixtures"
New-Item -ItemType Directory -Force $d | Out-Null

& $ff -y -v error -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=10" -c:v libx264 -pix_fmt yuv420p "$d\long_1080p.mp4"
& $ff -y -v error -f lavfi -i "testsrc2=size=720x1280:rate=25:duration=5" -f lavfi -i "sine=frequency=440:duration=5" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$d\vertical_audio.mp4"
& $ff -y -v error -f lavfi -i "testsrc2=size=480x270:rate=15:duration=4" "$d\anim.gif"
& $ff -y -v error -f lavfi -i "testsrc2=size=800x600:duration=0.04:rate=25" -frames:v 1 "$d\photo.png"

Get-ChildItem $d | Select-Object Name, @{n='KB';e={[math]::Round($_.Length/1KB)}}
