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
# все источники закреплены (коммит/тег + sha256) — сборка воспроизводима
source "$(dirname "$0")/ffmpeg-version.sh"
VPX_VER=1.15.2
VPX_SHA256=26fcd3db88045dee380e581862a6ef106f49b74b6396ee95c2993a260b4636aa
DAV1D_VER=1.5.3
DAV1D_SHA256=cbe212b02faf8c6eed5b6d55ef8a6e363aaab83f15112e960701a9c3df813686

mkdir -p "$SRC"
cd "$SRC"

# ---------- libvpx ----------
if [ ! -f "$SRC/vpx/lib/libvpx.a" ]; then
  fetch_verified "https://github.com/webmproject/libvpx/archive/refs/tags/v$VPX_VER.tar.gz" \
    "$VPX_SHA256" libvpx.tar.gz "libvpx-$VPX_VER"
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

# ---------- dav1d (программный AV1-декодер) ----------
# встроенный av1 в ffmpeg — только hardware-обёртка, без hwaccel не работает
# guard — по последнему создаваемому артефакту (иначе оборванный прогон
# оставит libdav1d.a без dav1d.pc и блок будет ошибочно пропущен)
if [ ! -f "$SRC/dav1d/lib/pkgconfig/dav1d.pc" ]; then
  if [ ! -f "dav1d-src/.sticker-nah-verified" ]; then
    rm -rf dav1d-src "dav1d-$DAV1D_VER"
    curl -sL "https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D_VER/dav1d-$DAV1D_VER.tar.gz" -o dav1d.tar.gz
    echo "$DAV1D_SHA256 *dav1d.tar.gz" | sha256sum -c -
    tar xf dav1d.tar.gz
    mv "dav1d-$DAV1D_VER" dav1d-src
    touch dav1d-src/.sticker-nah-verified
  fi
  # meson — нативный windows-бинарь, пути в cross-файле нужны в windows-виде
  cat > "$SRC/android-cross.txt" <<EOF
[binaries]
c = '$(cygpath -m "$CC")'
ar = '$(cygpath -m "$TC/bin/llvm-ar.exe")'
strip = '$(cygpath -m "$TC/bin/llvm-strip.exe")'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF
  rm -rf dav1d-src/build
  # prefix фиктивный (posix — иначе meson отвергает), установка руками ниже:
  # MSYS2 конвертирует пути в C:/…, а meson при кросс-сборке под android
  # требует posix-путь; MSYS2_ARG_CONV_EXCL отключает конвертацию для --prefix
  MSYS2_ARG_CONV_EXCL="--prefix" meson setup dav1d-src/build dav1d-src \
    --cross-file "$SRC/android-cross.txt" \
    --prefix=/dav1d \
    --default-library=static --buildtype release \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false
  ninja -C dav1d-src/build
  mkdir -p "$SRC/dav1d/lib/pkgconfig" "$SRC/dav1d/include/dav1d"
  cp dav1d-src/build/src/libdav1d.a "$SRC/dav1d/lib/"
  cp dav1d-src/include/dav1d/*.h "$SRC/dav1d/include/dav1d/"
  cat > "$SRC/dav1d/lib/pkgconfig/dav1d.pc" <<EOF
prefix=$SRC/dav1d
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: dav1d
Description: AV1 decoding library
Version: 1.5.3
Libs: -L\${libdir} -ldav1d
Cflags: -I\${includedir}
EOF
fi

# ---------- ffmpeg ----------
fetch_verified "https://github.com/FFmpeg/FFmpeg/archive/$FFMPEG_COMMIT.tar.gz" \
  "$FFMPEG_SHA256" ffmpeg-src.tar.gz "FFmpeg-$FFMPEG_COMMIT"
cd "FFmpeg-$FFMPEG_COMMIT"
make distclean >/dev/null 2>&1 || true

export PKG_CONFIG_PATH="$SRC/dav1d/lib/pkgconfig"
./configure \
  --enable-cross-compile --target-os=android --arch=aarch64 \
  --cc="$CC" --cxx="$CXX" --strip="$TC/bin/llvm-strip.exe" \
  --ar="$TC/bin/llvm-ar.exe" --ranlib="$TC/bin/llvm-ranlib.exe" --nm="$TC/bin/llvm-nm.exe" \
  --extra-cflags="-I$SRC/vpx/include -I$SRC/dav1d/include -fPIC" \
  --extra-ldflags="-L$SRC/vpx/lib -L$SRC/dav1d/lib" \
  --pkg-config=pkg-config \
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
  --pkg-config-flags='--static'

make -j"$(nproc)"
"$TC/bin/llvm-strip.exe" ffmpeg ffprobe
mkdir -p "$OUT"
cp ffmpeg "$OUT/libffmpeg.so"
cp ffprobe "$OUT/libffprobe.so"
# в APK попадают копии из jniLibs — обновляем их сразу, если проект уже инициализирован
JNI="$(dirname "$OUT")/../gen/android/app/src/main/jniLibs/arm64-v8a"
if [ -d "$JNI" ]; then
  cp "$OUT/libffmpeg.so" "$OUT/libffprobe.so" "$JNI/"
fi
ls -la "$OUT"
