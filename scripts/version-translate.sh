#!/bin/sh
# Translate native semver (X.Y.Z[-dev.N]) to packager-specific syntax.
#
# Usage: version-translate.sh <format> <native-version>
#   <format>: opkg | apk | native
#
# Examples:
#   1.1.0        -> opkg   1.1.0       apk 1.1.0        native 1.1.0
#   1.2.0-dev.1  -> opkg   1.2.0~dev1  apk 1.2.0_alpha1 native 1.2.0-dev.1
set -eu

format="${1:-}"
ver="${2:-}"

if [ -z "$format" ] || [ -z "$ver" ]; then
    echo "usage: $0 <opkg|apk|native> <version>" >&2
    exit 2
fi

# Validate shape: X.Y.Z or X.Y.Z-dev.N (X/Y/Z/N all non-negative ints)
case "$ver" in
    [0-9]*.[0-9]*.[0-9]*-dev.[0-9]*) is_dev=1 ;;
    [0-9]*.[0-9]*.[0-9]*)            is_dev=0 ;;
    *)  echo "error: unrecognized version format: $ver" >&2; exit 1 ;;
esac

if [ "$is_dev" -eq 0 ]; then
    # Stable version: identical in every format.
    printf '%s\n' "$ver"
    exit 0
fi

# Dev version: split on "-dev." to get base and counter.
base="${ver%-dev.*}"
counter="${ver##*-dev.}"

case "$format" in
    native) printf '%s\n' "$ver" ;;
    opkg)   printf '%s~dev%s\n' "$base" "$counter" ;;
    apk)    printf '%s_alpha%s\n' "$base" "$counter" ;;
    *)      echo "error: unknown format: $format" >&2; exit 1 ;;
esac
