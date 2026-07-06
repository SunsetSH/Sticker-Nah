#!/bin/bash
# Minimal static ffmpeg/ffprobe build for Sticker Nah (run inside MSYS2 MINGW64).
# Only: input demuxers/decoders for photo/gif/video, libvpx VP8/VP9 encoders,
# webm muxer, 5 filters. No audio codecs, no network, no docs.
# Требуется: pacman -S mingw-w64-x86_64-dav1d (программный AV1-декодер —
# встроенный av1 в ffmpeg только hardware-обёртка и без hwaccel не работает).
set -euo pipefail

# master-снапшот: декодер webp_anim (анимированный WebP) ещё не попал в релизы
SRC=/tmp/ffmpeg-src
OUT="$(cygpath -u "$1")" # куда положить готовые exe

mkdir -p "$SRC"
cd "$SRC"
if [ ! -d "FFmpeg-master" ]; then
  curl -sL "https://github.com/FFmpeg/FFmpeg/archive/refs/heads/master.tar.gz" -o ffmpeg-master.tar.gz
  tar xf ffmpeg-master.tar.gz
fi
cd FFmpeg-master

./configure \
  --disable-everything --disable-autodetect --disable-network --disable-doc \
  --disable-avdevice --disable-debug --disable-ffplay \
  --enable-gpl --enable-libvpx --enable-libdav1d --enable-small \
  --enable-zlib \
  --enable-protocol=file,pipe \
  --enable-demuxer=mov,matroska,gif,apng,avi,image2,image2pipe,image_png_pipe,image_jpeg_pipe,image_bmp_pipe,image_webp_pipe,webp_anim \
  --enable-decoder=h264,hevc,mpeg4,mjpeg,vp8,vp9,libdav1d,png,gif,webp,webp_anim,bmp,apng \
  --enable-parser=h264,hevc,vp9,png,mjpeg,gif,av1 \
  --enable-encoder=libvpx_vp9,libvpx_vp8 \
  --enable-muxer=webm,null \
  --enable-bsf=vp9_superframe \
  --enable-filter=scale,crop,fps,format,setpts,null,copy \
  --extra-ldflags='-static' --pkg-config-flags='--static'

make -j"$(nproc)"
strip ffmpeg.exe ffprobe.exe
mkdir -p "$OUT"
cp ffmpeg.exe ffprobe.exe "$OUT/"
ls -la "$OUT"/ffmpeg.exe "$OUT"/ffprobe.exe
