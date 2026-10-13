#!/usr/bin/env bash
# Workstation builds of must-gather, one static binary per platform, for
# the GitHub release (stormcos_qa#48):
#   sc-build deploy/build-release.sh
#
# Builds every target in TARGETS that this toolchain has, writes
# dist/must-gather-<version>-<target>[.exe], dist/gather-<version>.tar.gz
# (the component collector scripts) and dist/SHA256SUMS, and says which
# targets it could not build and why. The two Apple targets are
# cross-compiled (a macOS SDK on the box, stormcentral#387) and merged by
# llvm-lipo into one universal binary. The same recipe as storminstall's
# and sc's deploy/build-release.sh.
#
# This is not the golden: `stormcentral component build stormgather`
# builds bin/must-gather for x86_64 musl only.
set -uo pipefail
cd "$(dirname "$0")/.."
TARGETS=${TARGETS:-"x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu x86_64-apple-darwin aarch64-apple-darwin"}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TARGET_DIR=${CARGO_TARGET_DIR:-target}
installed=$(rustup target list --installed 2>/dev/null)
# ring has C; the build box has no aarch64 gcc, but clang cross-compiles it.
export CC_aarch64_unknown_linux_musl=${CC_aarch64_unknown_linux_musl:-clang}
export CFLAGS_aarch64_unknown_linux_musl=${CFLAGS_aarch64_unknown_linux_musl:---target=aarch64-linux-musl}
# Windows: mingw links it (x86_64-w64-mingw32-gcc is on the build box).
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}
# macOS: osxcross's wrappers if the box has them (they carry the SDK);
# otherwise clang + ld64.lld against $SDKROOT; `cargo zigbuild` wins if
# installed (zig links Mach-O, still against $SDKROOT).
for a in x86_64:o64-clang aarch64:oa64-clang; do
  arch=${a%%:*} wrap=${a#*:} var=CARGO_TARGET_${a%%:*}_APPLE_DARWIN_LINKER
  var=${var^^}
  if [[ -z ${!var:-} ]]; then
    if command -v "$wrap" >/dev/null; then
      export "$var=$wrap" "CC_${arch}_apple_darwin=$wrap"
    elif [[ -n ${SDKROOT:-} ]]; then
      export "$var=clang"
      export "CARGO_TARGET_${arch^^}_APPLE_DARWIN_RUSTFLAGS=-Clink-arg=-fuse-ld=lld -Clink-arg=--target=$arch-apple-macos11 -Clink-arg=-isysroot$SDKROOT"
      export "CC_${arch}_apple_darwin=clang" "CFLAGS_${arch}_apple_darwin=--target=$arch-apple-macos11 -isysroot $SDKROOT"
    fi
  fi
done
apple_ready() { command -v cargo-zigbuild >/dev/null || command -v o64-clang >/dev/null || [[ -n ${SDKROOT:-} ]]; }
mkdir -p dist
built=() missing=() darwin=()
for t in $TARGETS; do
  if ! grep -qx "$t" <<<"$installed"; then
    missing+=("$t (target not installed in this toolchain)")
    continue
  fi
  build=build
  if [[ $t == *apple-darwin ]]; then
    if ! apple_ready; then
      missing+=("$t (no macOS SDK or cross linker on this box: stormcentral#387)")
      continue
    fi
    command -v cargo-zigbuild >/dev/null && build=zigbuild
  fi
  if ! cargo $build --locked --release --target "$t" -p must-gather; then
    missing+=("$t (build failed, above)")
    continue
  fi
  ext=""; [[ $t == *windows* ]] && ext=".exe"
  out="dist/must-gather-$VERSION-$t$ext"
  cp "$TARGET_DIR/$t/release/must-gather$ext" "$out"
  built+=("$out")
  [[ $t == *apple-darwin ]] && darwin+=("$out")
done
# One macOS download for both Macs (Intel and Apple silicon).
if [[ ${#darwin[@]} -eq 2 ]]; then
  uni="dist/must-gather-$VERSION-universal-apple-darwin"
  if llvm-lipo -create -output "$uni" "${darwin[@]}"; then
    built+=("$uni")
  else
    missing+=("universal-apple-darwin (llvm-lipo failed, above)")
  fi
fi
# The collector scripts, for --collectors-dir.
tar -czf "dist/gather-$VERSION.tar.gz" gather
(cd dist && sha256sum must-gather-* gather-* > SHA256SUMS)
echo "== must-gather $VERSION"
for b in "${built[@]}"; do
  printf 'built   %s  %s\n' "$(du -h "$b" | cut -f1)" "$b"
  file "$b" 2>/dev/null | sed 's/^/        /'
done
for m in "${missing[@]}"; do echo "missing $m"; done
cat dist/SHA256SUMS
(cd dist && sha256sum -c SHA256SUMS) || exit 1
# The native one runs here: a smoke test of the release binary.
native="dist/must-gather-$VERSION-x86_64-unknown-linux-musl"
[[ -x $native ]] && { "$native" --version || exit 1; }
[[ ${#missing[@]} -eq 0 ]]
