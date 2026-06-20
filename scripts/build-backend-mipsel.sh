#!/bin/sh
# Cross-build modem-interface for OpenWrt ramips/mt7621 (mipsel_24kc).
# Item #31 Phase 2 — this script carries the entire size pass: the
# release-mipsel cargo profile (fat LTO, codegen-units=1), the build-std
# optimize_for_size feature, and the immediate-abort panic strategy (target-
# scoped RUSTFLAGS -Cpanic=immediate-abort; on this nightly it is a real panic
# strategy, no longer a build-std feature). See the Phase 2 banner in
# docs/superpowers/plans/2026-06-09-item-31-lightweight-mips-build.md.
#
# Requires: Linux x86_64 host, pinned nightly + rust-src, and the OpenWrt
# 22.03 ramips/mt7621 SDK (GCC 11.2.0 musl).
#
# Usage:
#   OPENWRT_SDK=/abs/path/to/openwrt-sdk-22.03.7-ramips-mt7621_gcc-11.2.0_musl.Linux-x86_64 \
#     sh scripts/build-backend-mipsel.sh
#
# Environment:
#   OPENWRT_SDK          (required) unpacked SDK directory.
#   NIGHTLY              rustup channel to build with. Default:
#                        nightly-2026-06-09 (the dated channel that resolves to
#                        the proven build on fresh installs). On the CI server
#                        the proven build is installed as plain `nightly`, so
#                        use NIGHTLY=nightly there.
#   ALLOW_NIGHTLY_DRIFT  set to 1 to skip the rustc version-string assertion.
#                        Size/behavior numbers from a drifted nightly are NOT
#                        comparable to the Phase-2 gate record.
set -e

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="mipsel-unknown-linux-musl"
NIGHTLY="${NIGHTLY:-nightly-2026-06-09}"
# The toolchain proven in Phase 0/1 on the ZBT-WG3526 bench.
PINNED_RUSTC="1.98.0-nightly (cb46fbb8c 2026-06-08)"

# --- 1. Toolchain pin: assert the exact proven rustc build --------------------
set +e
RUSTC_VERSION="$(rustc "+$NIGHTLY" --version 2>&1)"
status=$?
set -e
if [ "$status" -ne 0 ]; then
  fail "rustc '+$NIGHTLY' is not runnable (try: rustup toolchain install $NIGHTLY):
$RUSTC_VERSION"
fi

case "$RUSTC_VERSION" in
  *"$PINNED_RUSTC"*)
    echo "Toolchain pin OK: $RUSTC_VERSION"
    ;;
  *)
    if [ "${ALLOW_NIGHTLY_DRIFT:-0}" = "1" ]; then
      echo "WARNING: ALLOW_NIGHTLY_DRIFT=1 — building with '$RUSTC_VERSION'" >&2
      echo "         (proven toolchain is 'rustc $PINNED_RUSTC'; results are not comparable)" >&2
    else
      fail "toolchain '$NIGHTLY' is '$RUSTC_VERSION', not the proven 'rustc $PINNED_RUSTC'.
Install the pinned nightly, point NIGHTLY at the channel that has it, or set
ALLOW_NIGHTLY_DRIFT=1 to build anyway (gate numbers are then not comparable)."
    fi
    ;;
esac

# build-std compiles libstd from source, so the rust-src component must exist.
SYSROOT="$(rustc "+$NIGHTLY" --print sysroot)"
if [ ! -d "$SYSROOT/lib/rustlib/src/rust/library" ]; then
  fail "rust-src component missing for '$NIGHTLY' (build-std needs it).
Run: rustup component add rust-src --toolchain $NIGHTLY"
fi

# --- 2. OpenWrt SDK cross toolchain ------------------------------------------
: "${OPENWRT_SDK:?set OPENWRT_SDK to the unpacked OpenWrt ramips/mt7621 SDK dir}"
[ -d "$OPENWRT_SDK" ] || fail "OPENWRT_SDK '$OPENWRT_SDK' is not a directory"

# Glob must resolve to exactly one toolchain dir; if it expands to several
# (or none) the -d test below fails with the path we tried.
TC="$(echo "$OPENWRT_SDK"/staging_dir/toolchain-*/bin)"
[ -d "$TC" ] || fail "no single toolchain bin dir at '$TC' (expected \$OPENWRT_SDK/staging_dir/toolchain-*/bin)"
CROSS_GCC="$TC/mipsel-openwrt-linux-musl-gcc"
CROSS_STRIP="$TC/mipsel-openwrt-linux-musl-strip"
[ -x "$CROSS_GCC" ] || fail "cross compiler not found/executable: $CROSS_GCC"
[ -x "$CROSS_STRIP" ] || fail "cross strip not found/executable: $CROSS_STRIP"

export PATH="$TC:$PATH"
# Silences the SDK wrappers' "STAGING_DIR not defined" warning.
export STAGING_DIR="$OPENWRT_SDK/staging_dir"
export CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER=mipsel-openwrt-linux-musl-gcc
export CC_mipsel_unknown_linux_musl=mipsel-openwrt-linux-musl-gcc
export AR_mipsel_unknown_linux_musl=mipsel-openwrt-linux-musl-ar
# On this nightly panic_immediate_abort is a real panic strategy, not a build-std
# feature; target-scoped so host build scripts and other arches are untouched.
export CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort"

# --- 3. Frontend dist (embedded into the binary at compile time) --------------
if [ -f "$ROOT/frontend/dist/index.html" ]; then
  echo "Using existing frontend/dist as-is (embedded by the embedded-frontend feature)."
  # The dist becomes part of the recorded binary size, so staleness must be
  # visible in the log.
  echo "frontend/dist/index.html mtime: $(ls -ld "$ROOT/frontend/dist/index.html")"
elif command -v npm >/dev/null 2>&1; then
  echo "frontend/dist missing — building it with npm..."
  (cd "$ROOT/frontend" && npm run build)
else
  fail "frontend/dist/index.html is missing and npm is not on PATH.
This host cannot build the frontend; ship a prebuilt frontend/dist/ directory
to '$ROOT/frontend/dist' (it is embedded into the binary at compile time)."
fi

# --- 4. Build (release-mipsel profile + build-std optimize_for_size) ----------
cd "$ROOT/backend"
cargo "+$NIGHTLY" build \
  -Z build-std=std,panic_abort \
  -Z build-std-features=optimize_for_size \
  --target "$TARGET" \
  --profile release-mipsel \
  --no-default-features --features real-hardware,tls,embedded-frontend

BIN="target/$TARGET/release-mipsel/modem-interface"
[ -f "$BIN" ] || fail "build did not produce $BIN"

# --- 5. SDK strip to a SEPARATE file (keep the cargo output untouched) --------
# [profile.release] already has strip=true, but the recorded gate number must
# come from the SDK strip — cargo's strip and the SDK strip may differ.
STRIPPED="$BIN.stripped"
cp "$BIN" "$STRIPPED"
"$CROSS_STRIP" -s "$STRIPPED"

# --- 6. Assert dynamically-linked MIPS32 PIE (the intended Phase-0 shape) -----
# Phase 0 produced: ELF 32-bit LSB pie executable, MIPS, MIPS32 rel2 ...,
# dynamically linked, interpreter /lib/ld-musl-mipsel-sf.so.1. A static or
# static-pie binary has no "interpreter" clause and must fail here.
command -v file >/dev/null 2>&1 || fail "file(1) not found; cannot verify the binary shape"
FILE_OUT="$(file -b "$STRIPPED")"
for needle in "pie executable" "MIPS32" "interpreter "; do
  case "$FILE_OUT" in
    *"$needle"*) ;;
    *) fail "binary is not a dynamically-linked MIPS32 PIE (missing '$needle'):
$FILE_OUT" ;;
  esac
done
echo "Binary shape OK: dynamically-linked MIPS32 PIE."

# --- 7. Size record (the Phase-2 gate numbers) ---------------------------------
# In POSIX sh the pipeline status comes from wc, so a missing gzip would
# silently record 0 as a gate number — check for it up front.
command -v gzip >/dev/null 2>&1 || fail "gzip not found; cannot produce the jffs2 size proxy"
UNSTRIPPED_BYTES="$(wc -c < "$BIN")"
STRIPPED_BYTES="$(wc -c < "$STRIPPED")"
GZIP_BYTES="$(gzip -9 -c "$STRIPPED" | wc -c)"
echo ""
echo "=== Item #31 Phase 2 size record ==="
echo "cargo output: $UNSTRIPPED_BYTES bytes  backend/$BIN"
echo "SDK-stripped: $STRIPPED_BYTES bytes  backend/$STRIPPED"
echo "gzip -9:      $GZIP_BYTES bytes  (cheap proxy for the jffs2 on-flash footprint)"
echo "file:         $FILE_OUT"
ls -l "$BIN" "$STRIPPED"
