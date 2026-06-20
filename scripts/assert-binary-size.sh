#!/bin/sh
# CI size-regression guard for the shipping binaries.
#
# Fails the build if a binary exceeds a fixed byte ceiling. In-field upgrades
# happen via `apk add` while the old binary is still installed, so apk needs
# free overlay space for BOTH binaries transiently (~2x the binary size). On
# space-constrained overlays a silent footprint regrowth is a hard upgrade
# blocker (surfaced in Phase-5a: a 36MB overlay was 100% full). This gate keeps
# the post-slimming-sweep footprint from creeping back up unnoticed. Mirrors the
# mipsel jffs2-proxy assertion in scripts/build-backend-mipsel.sh (that target
# has its own assertion — do NOT wire this one for mipsel).
#
# Usage: assert-binary-size.sh <binary-path> <max-bytes> <label>
#   <binary-path>  path to the (stripped) shipping binary
#   <max-bytes>    inclusive upper bound, in bytes (decimal integer)
#   <label>        human label for log output (e.g. "aarch64")
#
# Exit: 0 if size <= max-bytes; non-zero (1) if over; 2 on usage/arg error.
set -eu

bin="${1:-}"
max="${2:-}"
label="${3:-}"

if [ -z "$bin" ] || [ -z "$max" ] || [ -z "$label" ]; then
    echo "usage: $0 <binary-path> <max-bytes> <label>" >&2
    exit 2
fi

# max-bytes must be a positive decimal integer.
case "$max" in
    ''|*[!0-9]*) echo "error: max-bytes must be a non-negative integer: '$max'" >&2; exit 2 ;;
esac

if [ ! -f "$bin" ]; then
    echo "error: binary not found: $bin" >&2
    exit 2
fi

actual="$(wc -c < "$bin")"
# wc may emit leading whitespace on some platforms; strip it.
actual="$(printf '%s' "$actual" | tr -d '[:space:]')"

if [ "$actual" -le "$max" ]; then
    echo "OK:   $label binary $actual bytes <= ceiling $max bytes ($bin)"
    exit 0
fi

over=$((actual - max))
echo "ERROR: $label binary is OVER the size ceiling." >&2
echo "  binary:  $bin" >&2
echo "  actual:  $actual bytes" >&2
echo "  ceiling: $max bytes" >&2
echo "  over by: $over bytes" >&2
echo "" >&2
echo "The shipping binary grew past its CI ceiling. Investigate the dependency" >&2
echo "graph for a newly-pulled heavy crate, or — if the growth is justified —" >&2
echo "raise the ceiling in .woodpecker.yml with a rationale comment." >&2
exit 1
