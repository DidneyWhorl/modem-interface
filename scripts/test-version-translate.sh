#!/bin/sh
# POSIX-sh test harness. No framework — this is ops glue.
set -eu

run() {
    expected="$1"; shift
    got="$("$@")"
    if [ "$got" != "$expected" ]; then
        echo "FAIL: $* -> '$got' (want '$expected')"
        exit 1
    fi
    echo "OK:   $* -> $got"
}

T=./scripts/version-translate.sh

# Stable versions pass through unchanged everywhere.
run "1.1.0"       "$T" opkg   "1.1.0"
run "1.1.0"       "$T" apk    "1.1.0"
run "1.1.0"       "$T" native "1.1.0"

# Hotfix versions pass through.
run "1.1.3"       "$T" opkg   "1.1.3"
run "1.1.3"       "$T" apk    "1.1.3"

# Dev versions translate per packager.
run "1.2.0~dev1"  "$T" opkg   "1.2.0-dev.1"
run "1.2.0_alpha1" "$T" apk   "1.2.0-dev.1"
run "1.2.0-dev.1" "$T" native "1.2.0-dev.1"

# Multi-digit dev counter.
run "1.2.0~dev17" "$T" opkg   "1.2.0-dev.17"
run "1.2.0_alpha17" "$T" apk  "1.2.0-dev.17"

# Unknown format exits non-zero.
if "$T" opkg "not-a-version" 2>/dev/null; then
    echo "FAIL: unknown format should exit non-zero"
    exit 1
fi
echo "OK:   unknown format rejected"

echo "=== all version-translate tests passed ==="
