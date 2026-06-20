#!/bin/sh
# POSIX-sh test harness for assert-binary-size.sh. No framework — ops glue,
# mirrors scripts/test-version-translate.sh.
set -eu

S=./scripts/assert-binary-size.sh

TMPDIR_T="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_T"' EXIT INT TERM

# A binary of EXACTLY 1000 bytes.
BIN="$TMPDIR_T/fake-binary"
# `head -c` is non-POSIX; build the file portably with dd.
dd if=/dev/zero of="$BIN" bs=1 count=1000 >/dev/null 2>&1
size="$(wc -c < "$BIN" | tr -d '[:space:]')"
if [ "$size" != "1000" ]; then
    echo "FAIL: test fixture is $size bytes, expected 1000"
    exit 1
fi

pass() {
    desc="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "OK:   $desc"
    else
        echo "FAIL: $desc (expected exit 0, got $?)"
        exit 1
    fi
}

fail() {
    desc="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "FAIL: $desc (expected non-zero exit, got 0)"
        exit 1
    fi
    echo "OK:   $desc"
}

# Under the limit -> PASS (1000 <= 2000).
pass "under-limit passes"            sh "$S" "$BIN" 2000 testlabel
# Exactly at the limit -> PASS (1000 <= 1000), boundary is inclusive.
pass "at-limit passes (inclusive)"   sh "$S" "$BIN" 1000 testlabel
# Over the limit -> FAIL (1000 > 999).
fail "over-limit fails"              sh "$S" "$BIN" 999  testlabel
# Missing binary -> error.
fail "missing binary errors"         sh "$S" "$TMPDIR_T/nope" 2000 testlabel
# Non-integer ceiling -> usage error.
fail "non-integer ceiling errors"    sh "$S" "$BIN" "abc" testlabel
# Missing args -> usage error.
fail "missing args errors"           sh "$S" "$BIN"

echo "=== all assert-binary-size tests passed ==="
