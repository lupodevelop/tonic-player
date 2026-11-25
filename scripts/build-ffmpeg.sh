#!/usr/bin/env bash
# Builds ffmpeg and ffprobe for one Rust target triple with only the formats,
# codecs and filters the app uses, linked statically against libx264 and
# libdav1d built from pinned sources. Apple and Linux targets build natively;
# x86_64-pc-windows-msvc is cross-built with mingw-w64 on Linux.
#
#   scripts/build-ffmpeg.sh <target-triple> [ffmpeg-version]
#
# Writes ffmpeg-<triple>[.exe], ffprobe-<triple>[.exe] and the source tarballs
# to OUT_DIR (default ./ffmpeg-out). JOBS sets make parallelism.
# Needs a C compiler, make, pkg-config, meson, ninja and, on x86_64, nasm.
set -euo pipefail

target=${1:?usage: build-ffmpeg.sh <target-triple> [ffmpeg-version]}
version=${2:-9.1}
out=$(mkdir -p "${OUT_DIR:-ffmpeg-out}" && cd "${OUT_DIR:-ffmpeg-out}" && pwd)
jobs=${JOBS:-$(getconf _NPROCESSORS_ONLN)}

case $version in
  9.1) ffmpeg_sha256=1a4d394ae63d190a526cd052eb4415c156b5c5e2df37efab41a7de0b8fde2433 ;;
  *) echo "No sha256 pinned for FFmpeg $version; add it to $0" >&2; exit 1 ;;
esac
x264_commit=b35605ace3ddf7c1a5d67a2eb553f034aef41d55 # head of the stable branch
x264_sha256=6eeb82934e69fd51e043bd8c5b0d152839638d1ce7aa4eea65a3fedcf83ff224
dav1d_version=1.5.5
dav1d_sha256=ca96b1d0ba3f01f55de7231d775892b64994ba061aaeeaaa4f662a390b3abf9e

ffmpeg_tar=ffmpeg-$version.tar.xz
x264_tar=x264-$x264_commit.tar.bz2
dav1d_tar=dav1d-$dav1d_version.tar.xz

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
prefix=$work/prefix

exe=
ffmpeg_cross=()
x264_cross=()
dav1d_cross=()
ldflags=
case $target in
  aarch64-apple-darwin)
    export MACOSX_DEPLOYMENT_TARGET=11.0 ;;
  x86_64-apple-darwin)
    export MACOSX_DEPLOYMENT_TARGET=10.13 ;;
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) ;;
  x86_64-pc-windows-msvc)
    exe=.exe
    host=x86_64-w64-mingw32
    ffmpeg_cross=(--enable-cross-compile --cross-prefix=$host- --arch=x86_64 --target-os=mingw32)
    x264_cross=(--host=$host --cross-prefix=$host-)
    dav1d_cross=(--cross-file=package/crossfiles/$host.meson)
    # No libgcc or winpthread DLLs next to the exe.
    ldflags=-static ;;
  *) echo "Unsupported target $target" >&2; exit 1 ;;
esac

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

fetch() { # url file sha256
  curl -fsSL --retry 3 -o "$out/$2" "$1"
  local sum
  sum=$(sha256 "$out/$2")
  [ "$sum" = "$3" ] || { echo "$2: sha256 $sum, expected $3" >&2; exit 1; }
  mkdir -p "$work/src"
  tar -xf "$out/$2" -C "$work/src"
}

fetch "https://ffmpeg.org/releases/$ffmpeg_tar" "$ffmpeg_tar" "$ffmpeg_sha256"
fetch "https://code.videolan.org/videolan/x264/-/archive/$x264_commit/$x264_tar" "$x264_tar" "$x264_sha256"
fetch "https://downloads.videolan.org/pub/videolan/dav1d/$dav1d_version/$dav1d_tar" "$dav1d_tar" "$dav1d_sha256"

# Only the libraries built here are visible to pkg-config, never the host's.
export PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig

(
  cd "$work/src/x264-$x264_commit"
  ./configure --prefix="$prefix" ${x264_cross[@]+"${x264_cross[@]}"} --enable-static --enable-pic --disable-cli \
    --disable-opencl --disable-avs --disable-swscale --disable-lavf --disable-ffms --disable-gpac --disable-lsmash
  make -j"$jobs"
  make install
)

(
  cd "$work/src/dav1d-$dav1d_version"
  meson setup build ${dav1d_cross[@]+"${dav1d_cross[@]}"} --prefix="$prefix" --libdir=lib --buildtype=release \
    --default-library=static -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
  ninja -C build -j "$jobs" install
)

platform=()
if [[ $target == *apple-darwin ]]; then
  platform=(--enable-videotoolbox --enable-encoder=h264_videotoolbox)
fi

# The lavfi device, testsrc2, sine, wrapped_avframe, the ac3 encoder and the
# matroska and mp4 muxers are only there for the app's tests, which make their
# inputs with the bundled ffmpeg. The native av1 decoder decodes only through a
# hwaccel, so AV1 goes through libdav1d. The raw demuxers identify streams of
# an unknown type in MPEG-PS and MPEG-TS.
cd "$work/src/ffmpeg-$version"
./configure \
  ${ffmpeg_cross[@]+"${ffmpeg_cross[@]}"} \
  --pkg-config=pkg-config --pkg-config-flags=--static \
  --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib $ldflags" \
  --enable-gpl --enable-version3 \
  --disable-everything --disable-autodetect \
  --disable-doc --disable-debug --disable-ffplay \
  --enable-libx264 --enable-libdav1d \
  --enable-protocol=file,http,tcp,pipe \
  --enable-demuxer=mov,matroska,avi,mpegts,flv,asf,mpegps,ogg \
  --enable-demuxer=mpegvideo,m4v,h264,hevc,aac,loas,ac3,eac3,dts,truehd,mp3 \
  --enable-decoder=h264,hevc,vp8,vp9,libdav1d,mpeg4,mpeg1video,mpeg2video,h263,flv,vp6f,mjpeg \
  --enable-decoder=msmpeg4v1,msmpeg4v2,msmpeg4v3,wmv1,wmv2,wmv3,vc1,theora \
  --enable-decoder=aac,aac_latm,ac3,eac3,dca,truehd,mlp,mp1,mp2,mp3,mp1float,mp2float,mp3float \
  --enable-decoder=opus,vorbis,flac,alac,wmav1,wmav2,wmapro,amrnb,amrwb,'pcm_*',wrapped_avframe \
  --enable-parser=h264,hevc,vp8,vp9,av1,mpeg4video,mpegvideo,h263,vc1,vp3,mjpeg \
  --enable-parser=aac,aac_latm,ac3,dca,mlp,mpegaudio,opus,vorbis,flac \
  --enable-encoder=libx264,aac,mjpeg,ac3 \
  --enable-muxer=hls,mpegts,image2,image2pipe,matroska,mp4 \
  --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,aac_adtstoasc,extract_extradata \
  --enable-filter=scale,format,v360,aresample,aformat,null,anull,testsrc2,sine \
  --enable-indev=lavfi \
  ${platform[@]+"${platform[@]}"}
make -j"$jobs" "ffmpeg$exe" "ffprobe$exe"

# make leaves stripped ffmpeg/ffprobe next to the unstripped *_g.
for tool in ffmpeg ffprobe; do
  cp "$tool$exe" "$out/$tool-$target$exe"
done

cd "$out"
ls -l "ffmpeg-$target$exe" "ffprobe-$target$exe"
if [ -z "$exe" ]; then
  "./ffmpeg-$target" -hide_banner -buildconf
else
  grep FFMPEG_CONFIGURATION "$work/src/ffmpeg-$version/config.h"
fi
