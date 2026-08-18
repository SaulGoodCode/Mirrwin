#!/usr/bin/env bash
# Build airplay2dll.dll from upstream xenos1337/AirPlayServer plus the overlay
# in tools/airplay-dll/.
#
#   tools/build-airplay-dll.sh [path-to-upstream-checkout]
#
# The overlay swaps two things into the upstream tree and changes nothing else:
#
#   Bridge.cpp            the C ABI the Rust host calls (mirror_start_ex),
#                         including the connect/disconnect state callback
#   FgAirplayChannel.*    a forwarding channel with no FFmpeg decode path, so
#                         the H.264 elementary stream reaches the host as-is
#                         and avcodec/swscale/avutil are never linked
#
# Requires MSYS2 mingw64 at C:\msys64. Output: <upstream>/build/airplay2dll.dll
set -e

export TMP="${TMP:-/tmp}"
export TEMP="${TEMP:-/tmp}"
export TMPDIR="${TMPDIR:-/tmp}"

OVERLAY="$(cd "$(dirname "$0")/airplay-dll" && pwd)"
UPSTREAM_DIR="${1:-E:/tmp/xenos/AirPlayServer-research}"
UPSTREAM_URL="https://github.com/xenos1337/AirPlayServer"

if [[ ! -d "$UPSTREAM_DIR/AirPlayServerLib" ]]; then
  echo "==> cloning $UPSTREAM_URL -> $UPSTREAM_DIR"
  git clone --depth 1 "$UPSTREAM_URL" "$UPSTREAM_DIR"
fi

REPO="$(cd "$UPSTREAM_DIR" && pwd -W)"
OUT="$REPO/build"
OBJ="$OUT/obj"
MINGW="/c/msys64/mingw64"
export PATH="$MINGW/bin:$PATH"

CORE_VCX="$REPO/AirPlayServerLib/AirPlayLib.vcxproj"
DLL_VCX="$REPO/airplay2dll/airplay2dll.vcxproj"

echo "==> upstream: $REPO"
echo "==> overlay:  $OVERLAY"
echo "==> gcc:      $(gcc --version | head -1)"

# ---------- overlay ----------
cp "$OVERLAY/Bridge.cpp"            "$REPO/airplay2dll/src/Bridge.cpp"
cp "$OVERLAY/BridgeTap.h"           "$REPO/airplay2dll/BridgeTap.h"
cp "$OVERLAY/FgAirplayChannel.h"    "$REPO/airplay2dll/FgAirplayChannel.h"
cp "$OVERLAY/FgAirplayChannel.cpp"  "$REPO/airplay2dll/FgAirplayChannel.cpp"

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
# 4) upstream swallows its own startup failure: FgAirplayServer::start() returns
#    0 unconditionally and fgServerStartWithDisplay ignores the result, so a
#    receiver that never came up (port taken, Bonjour service down) still looked
#    started to the host. Make the failure reach the caller. Each edit is
#    verified below and the build fails if an anchor ever stops matching.
SRV="$REPO/airplay2dll/src/FgAirplayServer.cpp"
EXP="$REPO/airplay2dll/src/Airplay2Export.cpp"
if ! grep -q 'overlay: propagate startup failure' "$SRV"; then
  sed -i 's|^\t\tstop();$|\t\tstop();\n\t\treturn ret;  // overlay: propagate startup failure|' "$SRV"
fi
if ! grep -q 'overlay: do not hand back a dead server' "$EXP"; then
  sed -i 's|^\tpServer->start(serverName, raopPort, airplayPort, callback, password,$|\tint rc = pServer->start(serverName, raopPort, airplayPort, callback, password,|' "$EXP"
  sed -i 's|^\treturn pServer;$|\tif (rc != 0) {  // overlay: do not hand back a dead server\n\t\tdelete pServer;\n\t\treturn NULL;\n\t}\n\treturn pServer;|' "$EXP"
fi
grep -q 'overlay: propagate startup failure' "$SRV" || {
  echo "  ERROR: could not patch FgAirplayServer::start (upstream changed?)"; exit 1; }
grep -q 'overlay: do not hand back a dead server' "$EXP" || {
  echo "  ERROR: could not patch fgServerStartWithDisplay (upstream changed?)"; exit 1; }
grep -q 'int rc = pServer->start' "$EXP" || {
  echo "  ERROR: fgServerStartWithDisplay patch is half-applied"; exit 1; }
echo "==> upstream startup-failure patches applied"

# Deliberately no -I external/ffmpeg/include: a stray libav* include should
# fail the build rather than quietly restore the dependency we just removed.
INC=(
  -I"$REPO/airplay2dll"
  -I"$REPO/airplay2dll/include"
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

rm -rf "$OBJ"
mkdir -p "$OBJ"

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
  -L"$REPO/external/plist/lib/x64" -lplist \
  -lws2_32 -lwinmm -liphlpapi -lbcrypt -ldnsapi -lcrypt32 -lsecur32 -ladvapi32 -luser32 -lgdi32 -lole32 -luuid -lsetupapi -lwsock32 \
  -static-libgcc -static-libstdc++

echo "==> built: $OUT/airplay2dll.dll"
ls -la "$OUT/airplay2dll.dll"

echo "==> exports:"
objdump -p "$OUT/airplay2dll.dll" | grep -iE "mirror_start_ex|mirror_stop" || {
  echo "  ERROR: expected exports missing"; exit 1; }

echo "==> imports:"
objdump -p "$OUT/airplay2dll.dll" | grep -E "^\s+DLL Name" | sort -u
if objdump -p "$OUT/airplay2dll.dll" | grep -qiE "msys-2\.0\.dll|avcodec|avutil|swscale"; then
  echo "  ERROR: FFmpeg/MSYS dependency reintroduced"; exit 1
fi
echo "==> ok: no FFmpeg/MSYS runtime dependency"
