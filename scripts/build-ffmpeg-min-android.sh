#!/bin/bash
# Cross-build of minimal ffmpeg/ffprobe for Android arm64 (run inside MSYS2 MINGW64).
# Result: libffmpeg.so / libffprobe.so — исполняемые файлы, названные как библиотеки,
# чтобы попасть в nativeLibraryDir APK и запускаться exec'ом.
# Usage: build-ffmpeg-min-android.sh <NDK_DIR_WIN> <OUT_DIR_WIN>
set -euo pipefail

NDK="$(cygpath -u "$1")"
OUT="$(cygpath -u "$2")"
API=24
TC="$NDK/toolchains/llvm/prebuilt/windows-x86_64"
CC="$TC/bin/aarch64-linux-android$API-clang.cmd"
CXX="$TC/bin/aarch64-linux-android$API-clang++.cmd"
SRC=/tmp/ffmpeg-android
VPX_VER=1.15.2

mkdir -p "$SRC"
cd "$SRC"

# ---------- libvpx ----------
if [ ! -f "$SRC/vpx/lib/libvpx.a" ]; then
  if [ ! -d "libvpx-$VPX_VER" ]; then
    curl -sL "https://github.com/webmproject/libvpx/archive/refs/tags/v$VPX_VER.tar.gz" -o libvpx.tar.gz
    tar xf libvpx.tar.gz
  fi
  cd "libvpx-$VPX_VER"
  CC="$CC" CXX="$CXX" LD="$CC" AR="$TC/bin/llvm-ar.exe" \
  STRIP="$TC/bin/llvm-strip.exe" RANLIB="$TC/bin/llvm-ranlib.exe" \
  ./configure --target=arm64-android-gcc --prefix="$SRC/vpx" \
    --disable-examples --disable-tools --disable-docs --disable-unit-tests \
    --enable-static --disable-shared --enable-vp8 --enable-vp9 --enable-pic
  make -j"$(nproc)"
  make install
  cd "$SRC"
fi

# ---------- ffmpeg ----------
if [ ! -d "FFmpeg-master" ]; then
  cp -r /tmp/ffmpeg-src/FFmpeg-master . 2>/dev/null || {
    curl -sL "https://github.com/FFmpeg/FFmpeg/archive/refs/heads/master.tar.gz" -o ffmpeg-master.tar.gz
    tar xf ffmpeg-master.tar.gz
  }
fi
cd FFmpeg-master
make distclean >/dev/null 2>&1 || true

./configure \
  --enable-cross-compile --target-os=android --arch=aarch64 \
  --cc="$CC" --cxx="$CXX" --strip="$TC/bin/llvm-strip.exe" \
  --ar="$TC/bin/llvm-ar.exe" --ranlib="$TC/bin/llvm-ranlib.exe" --nm="$TC/bin/llvm-nm.exe" \
  --extra-cflags="-I$SRC/vpx/include -fPIC" \
  --extra-ldflags="-L$SRC/vpx/lib" \
  --disable-everything --disable-autodetect --disable-network --disable-doc \
  --disable-avdevice --disable-debug --disable-ffplay \
  --enable-gpl --enable-libvpx --enable-small \
  --enable-zlib \
  --enable-protocol=file,pipe \
  --enable-demuxer=mov,matroska,gif,apng,avi,image2,image2pipe,image_png_pipe,image_jpeg_pipe,image_bmp_pipe,image_webp_pipe,webp_anim \
  --enable-decoder=h264,hevc,mpeg4,mjpeg,vp8,vp9,av1,png,gif,webp,webp_anim,bmp,apng \
  --enable-parser=h264,hevc,vp9,png,mjpeg,gif,av1 \
  --enable-encoder=libvpx_vp9,libvpx_vp8 \
  --enable-muxer=webm,null \
  --enable-bsf=vp9_superframe \
  --enable-filter=scale,crop,fps,format,setpts,null,copy \
  --pkg-config-flags='--static'

make -j"$(nproc)"
"$TC/bin/llvm-strip.exe" ffmpeg ffprobe
mkdir -p "$OUT"
cp ffmpeg "$OUT/libffmpeg.so"
cp ffprobe "$OUT/libffprobe.so"
ls -la "$OUT"
