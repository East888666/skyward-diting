#!/usr/bin/env bash
# Static arm64 Android NTFS reader and its complete corresponding source.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd -- "$script_dir/.." && pwd)
bundle=${1:-"$repo_dir/target/ntfs3g"}
mkdir -p "$bundle"
bundle=$(cd -- "$bundle" && pwd)
ndk=${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}
: "${ndk:?Set ANDROID_NDK_HOME to the Android NDK root}"
compiler_dir="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
test -x "$compiler_dir/aarch64-linux-android30-clang"

# Required on the build host: make autoconf automake libtool pkg-config
# libgcrypt20-dev patch python3. No host libraries are linked into Android.
revision=7f0f841fc52cf719106c5c93bafe465004e36816
digest=ef4c631d943667885347047aaedf48443c96bf4979c1cf2576d94ef65ed90f46
archive="$bundle/upstream.tar.gz"
if [ -f "$repo_dir/upstream.tar.gz" ]; then
    cp "$repo_dir/upstream.tar.gz" "$archive"
fi
python3 - "$archive" "$revision" "$digest" <<'PY'
import hashlib, pathlib, sys, urllib.request
path, revision, digest = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
if not path.exists():
    data = urllib.request.urlopen(f"https://codeload.github.com/tuxera/ntfs-3g/tar.gz/{revision}", timeout=120).read()
else:
    data = path.read_bytes()
if hashlib.sha256(data).hexdigest() != digest:
    raise SystemExit("NTFS-3G source checksum mismatch")
path.write_bytes(data)
PY

work=$(mktemp -d "$bundle/build.XXXXXX")
tar -xf "$archive" -C "$work"
source_dir="$work/ntfs-3g-$revision"
patch -d "$source_dir" -p1 < "$script_dir/ntfs3g-android.patch"
cd "$source_dir"
./autogen.sh
export CC="$compiler_dir/aarch64-linux-android30-clang"
export AR="$compiler_dir/llvm-ar"
export RANLIB="$compiler_dir/llvm-ranlib"
export NM="$compiler_dir/llvm-nm"
export STRIP="$compiler_dir/llvm-strip"
export CFLAGS="-Os -fPIE"
export LDFLAGS="-static"
./configure --host=aarch64-linux-android --with-fuse=internal \
    --disable-shared --enable-static --enable-really-static \
    --disable-ntfsprogs --disable-plugins --disable-mount-helper \
    --disable-ldconfig --disable-library
make -j"${NTFS3G_JOBS:-4}" -C libntfs-3g
make -j"${NTFS3G_JOBS:-4}" -C libfuse-lite
make -j"${NTFS3G_JOBS:-4}" -C src ntfs-3g
"$STRIP" src/ntfs-3g -o "$bundle/ntfs-3g"
cp COPYING "$bundle/COPYING"

# Include the exact upstream source, Android patch and this build recipe
# in every recovery carrying the binary. Rebuild this archive offline by
# extracting it and running tools/build-ntfs3g.sh with the NDK configured.
mkdir -p "$work/corresponding-source/tools"
cp "$archive" "$work/corresponding-source/upstream.tar.gz"
cp "$script_dir/build-ntfs3g.sh" "$script_dir/ntfs3g-android.patch" "$work/corresponding-source/tools/"
tar -czf "$bundle/source.tar.gz" -C "$work" corresponding-source
echo "NTFS reader bundle: $bundle"
