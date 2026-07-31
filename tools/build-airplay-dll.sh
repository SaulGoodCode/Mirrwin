#!/usr/bin/env bash
set -e
# Ensure GCC can create temp files (otherwise it tries C:\Windows\)
export TMP="${TMP:-/tmp}"
export TEMP="${TEMP:-/tmp}"
export TMPDIR="${TMPDIR:-/tmp}"
REPO="$(cd "$(dirname "$0")" && pwd -W)"
OUT="$REPO/build"
OBJ="$OUT/obj"
FFLIB="$OUT/fflib"
MINGW="/c/msys64/mingw64"
export PATH="$MINGW/bin:$PATH"

CORE_VCX="$REPO/AirPlayServerLib/AirPlayLib.vcxproj"
DLL_VCX="$REPO/airplay2dll/airplay2dll.vcxproj"

echo "==> REPO=$REPO"
echo "==> gcc: $(gcc --version | head -1)"

# ---------- idempotent source patches (applied before compile) ----------
# 1) MSVC-only compat clock_gettime conflicts with MinGW winpthreads
if grep -q '#ifdef WIN32' "$REPO/AirPlayServerLib/lib/byteutils.c" 2>/dev/null; then
  sed -i 's/#ifdef WIN32/#if defined(WIN32) \&\& defined(_MSC_VER)/' "$REPO/AirPlayServerLib/lib/byteutils.c"
fi
# 2) unqualified min/max -> std:: (strict C++)
if grep -q 'min(10, max(0.1, fRatio))' "$REPO/airplay2dll/src/FgAirplayServer.cpp" 2>/dev/null; then
  sed -i 's/min(10, max(0.1, fRatio))/std::min(10.0f, std::max(0.1f, fRatio))/' "$REPO/airplay2dll/src/FgAirplayServer.cpp"
fi
# 3) any UTF-16 source/header -> UTF-8 (gcc cannot parse UTF-16)
find "$REPO" -type f \( -name '*.c' -o -name '*.h' -o -name '*.cpp' -o -name '*.hpp' \) | while read -r f; do
  if head -c2 "$f" | grep -q $'\xff\xfe'; then
    iconv -f UTF-16 -t UTF-8 "$f" > "$f.tmp" && mv "$f.tmp" "$f"
  fi
done

INC=(
  -I"$REPO/airplay2dll"
  -I"$REPO/airplay2dll/include"
  -I"$REPO/external/ffmpeg/include"
  -I"$REPO/AirPlayServerLib/include"
  -I"$REPO/AirPlayServerLib"
  -I"$REPO/AirPlayServerLib/lib"
  -I"$REPO/AirPlayServerLib/lib/ed25519"
  -I"$REPO/AirPlayServerLib/lib/crypto"
  -I"$REPO/AirPlayServerLib/lib/curve25519"
  -I"$REPO/AirPlayServerLib/lib/playfair"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libAACdec/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libAACenc/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libArithCoding/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libDRCdec/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libFDK/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libFDK/include/x86"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libMpegTPDec/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libMpegTPEnc/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libPCMutils/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libSACdec/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libSACenc/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libSBRdec/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libSBRenc/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/libSYS/include"
  -I"$REPO/AirPlayServerLib/lib/fdk-aac/win32"
  -I"$REPO/external"
  -I"$REPO/external/plist/include"
)

CFLAGS="-DAIRPLAYSERVER_EXPORTS -w -fpermissive -O2"
CXXFLAGS="$CFLAGS"

mkdir -p "$OBJ" "$FFLIB"
for n in avcodec swscale avutil; do
  cp "$REPO/external/ffmpeg/lib/x64/$n.lib" "$FFLIB/lib$n.a"
done

core_srcs=$(grep -o '<ClCompile Include="[^"]*"' "$CORE_VCX" | sed 's/<ClCompile Include="//;s/"//')
dll_srcs=$(grep -o '<ClCompile Include="[^"]*"' "$DLL_VCX"  | sed 's/<ClCompile Include="//;s/"//')
dll_srcs="$dll_srcs src/Bridge.cpp"

echo "==> compiling core lib ($(echo "$core_srcs" | wc -w) files)..."
i=0
for s in $core_srcs; do
  sfs="${s//\\//}"
  obj="$OBJ/core_$(echo "$s" | sed 's#[\/]#_#g').o"
  src="$REPO/AirPlayServerLib/$sfs"
  if [[ "$s" == *.c ]]; then
    gcc $CFLAGS "${INC[@]}" -c "$src" -o "$obj"
  else
    g++ $CXXFLAGS "${INC[@]}" -c "$src" -o "$obj"
  fi
  i=$((i+1))
done
echo "    compiled $i core objects"

echo "==> compiling dll sources..."
for s in $dll_srcs; do
  sfs="${s//\\//}"
  obj="$OBJ/dll_$(echo "$s" | sed 's#[\/]#_#g').o"
  src="$REPO/airplay2dll/$sfs"
  if [[ "$s" == *.c ]]; then
    gcc $CFLAGS "${INC[@]}" -c "$src" -o "$obj"
  else
    g++ $CXXFLAGS "${INC[@]}" -c "$src" -o "$obj"
  fi
done

echo "==> archiving core lib"
ar rcs "$OUT/libairplay.a" "$OBJ"/core_*.o

echo "==> linking airplay2dll.dll"
g++ -shared -o "$OUT/airplay2dll.dll" "$OBJ"/dll_*.o "$OUT/libairplay.a" \
  -L"$FFLIB" -lavcodec -lswscale -lavutil \
  -L"$REPO/external/plist/lib/x64" -lplist \
  -lws2_32 -lwinmm -liphlpapi -lbcrypt -ldnsapi -lcrypt32 -lsecur32 -ladvapi32 -luser32 -lgdi32 -lole32 -luuid -lsetupapi -lwsock32 -static-libgcc -static-libstdc++

echo "==> built: $OUT/airplay2dll.dll"
ls -la "$OUT/airplay2dll.dll"
echo "==> exports:"
nm -C -D "$OUT/airplay2dll.dll" 2>/dev/null | grep -iE "mirror_start|mirror_stop|fgServerStart|fgServerStop" || \
  objdump -p "$OUT/airplay2dll.dll" 2>/dev/null | grep -iE "mirror_start|mirror_stop|fgServerStart|fgServerStop" || \
  echo "  (could not list exports)"
