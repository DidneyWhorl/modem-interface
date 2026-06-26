#!/bin/sh
# POSIX-sh test harness for publish-public.sh. No framework — ops glue,
# mirrors scripts/test-assert-binary-size.sh.
#
# Asserts the two safety contracts that matter for Phase C:
#   1. Known EXCLUDED paths NEVER appear in the staged export.
#   2. gitleaks is invoked FAIL-CLOSED: when gitleaks is absent (current state)
#      OR reports a finding, publish-public.sh aborts non-zero before any
#      commit/push.
#
# Strategy: drive publish-public.sh in --dry-run mode against the v1.4.0 tag and
# inspect its printed STAGED MANIFEST. The dry run aborts at the gitleaks gate
# (gitleaks absent) AFTER printing the staged manifest, which lets us assert
# both the exclusion contract and the fail-closed contract from one run.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
S="$SCRIPT_DIR/publish-public.sh"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

REF="${TEST_REF:-v1.4.0}"

# Resolve a ref to test against; fall back to HEAD if the tag is absent so the
# harness is still runnable on a checkout without the tag.
if ! git -C "$REPO_ROOT" rev-parse --verify --quiet "${REF}^{commit}" >/dev/null; then
    echo "note: ref '$REF' not found; falling back to HEAD"
    REF="HEAD"
fi

TMPDIR_T="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_T"' EXIT INT TERM

OUT="$TMPDIR_T/dryrun.out"
ERR="$TMPDIR_T/dryrun.err"

fails=0
note_fail() { echo "FAIL: $1"; fails=$((fails+1)); }
note_ok()   { echo "OK:   $1"; }

# --- Run the dry-run once; capture stdout, stderr, and exit code. ----------
set +e
sh "$S" --dry-run "$REF" >"$OUT" 2>"$ERR"
RC=$?
set -e

# Isolate just the staged manifest block (between the header and its footer).
MANIFEST_BLOCK="$TMPDIR_T/manifest.block"
awk '/^=== STAGED MANIFEST/{f=1;next} /^--- total staged files:/{f=0} f' "$OUT" > "$MANIFEST_BLOCK"

if [ ! -s "$MANIFEST_BLOCK" ]; then
    note_fail "dry-run did not print a staged manifest (cannot assert exclusions)"
    echo "----- stdout -----"; cat "$OUT"
    echo "----- stderr -----"; cat "$ERR"
    exit 1
fi
note_ok "dry-run printed a staged manifest ($(wc -l < "$MANIFEST_BLOCK" | tr -d '[:space:]') files)"

# --- Assertion 1: excluded paths NEVER appear in the staged manifest. ------
EXCLUDES="
docs/pm-memory/
.woodpecker.yml
graphify-out/
docs/ISSUES-AND-ROADMAP.md
CLAUDE.md
.codex/
AGENTS.md
scripts/setup-router-feed.ps1
scripts/deploy-
docs/CTRL-CLOUD-ARCHITECTURE.md
docs/superpowers/
frontend/tsconfig.tsbuildinfo
scripts/ci-build-all.ps1
scripts/generate-feed-index.sh
scripts/tls-gate-probe.sh
"
for ex in $EXCLUDES; do
    if grep -q -F -- "$ex" "$MANIFEST_BLOCK"; then
        note_fail "excluded path leaked into staged manifest: '$ex'"
        echo "      offending lines:"; grep -n -F -- "$ex" "$MANIFEST_BLOCK" | sed 's/^/        /'
    else
        note_ok "excluded path absent: '$ex'"
    fi
done

# --- Assertion 2: a couple of EXPECTED includes are present (sanity). ------
for inc in "backend/Cargo.toml" "frontend/" "openwrt/Makefile" "scripts/build-ipk.sh"; do
    if grep -q -F -- "$inc" "$MANIFEST_BLOCK"; then
        note_ok "expected include present: '$inc'"
    else
        note_fail "expected include MISSING from staged manifest: '$inc'"
    fi
done

# --- Assertion 3: gitleaks fail-closed contract. --------------------------
# With gitleaks absent (or any finding), the dry-run must exit non-zero AND must
# NOT print the "DRY RUN COMPLETE" success line.
if command -v gitleaks >/dev/null 2>&1; then
    echo "note: gitleaks is INSTALLED — testing the clean-scan + allowlist contract"
    # With gitleaks present AND the surgical allowlist (scripts/gitleaks-public.toml)
    # in place, the v1.4.0 staged tree scans clean: the env_cmd.rs localhost TLS
    # test fixture is allowlisted, the rest of the default ruleset stays active.
    # A clean tree must complete (rc 0), print the success line, and never
    # half-publish. (We do NOT weaken the fail-closed contract — see the absent
    # branch below; a real finding still aborts non-zero.)
    if [ "$RC" -eq 0 ]; then
        note_ok "gitleaks present: clean dry-run completed (rc 0)"
        if grep -q "DRY RUN COMPLETE" "$OUT"; then
            note_ok "gitleaks present: success line emitted (allowlist let the clean scan pass)"
        else
            note_fail "gitleaks present + rc 0 but no DRY RUN COMPLETE line"
        fi
        if grep -q "no leaks found" "$OUT" || grep -q "gitleaks: clean" "$OUT"; then
            note_ok "gitleaks present: scan reported clean (no findings)"
        else
            note_fail "gitleaks present + rc 0 but no clean-scan marker in output"
        fi
    else
        # Non-zero with gitleaks present means it found something — also a valid
        # fail-closed outcome, but flag it for human eyes. With the allowlist in
        # place against v1.4.0 this is NOT expected; surface the report.
        note_fail "gitleaks present: dry-run aborted non-zero (rc $RC) — UNEXPECTED with allowlist; check for a new finding"
        echo "----- stderr tail -----"; tail -20 "$ERR"
    fi
    # The allowlist config must live in the tooling only — it must NEVER appear
    # in the staged surface (it is intentionally absent from public-manifest.txt).
    if grep -q -F -- "scripts/gitleaks-public.toml" "$MANIFEST_BLOCK"; then
        note_fail "gitleaks allowlist config leaked into staged manifest: scripts/gitleaks-public.toml"
    else
        note_ok "gitleaks allowlist config absent from staged manifest (tooling-only)"
    fi
else
    # gitleaks ABSENT — must fail-closed: non-zero exit + abort message.
    if [ "$RC" -eq 0 ]; then
        note_fail "gitleaks ABSENT but dry-run exited 0 — NOT fail-closed!"
    else
        note_ok "gitleaks absent: dry-run aborted non-zero (rc $RC) — fail-closed"
    fi
    if grep -q "FAIL-CLOSED" "$ERR"; then
        note_ok "fail-closed abort message printed to stderr"
    else
        note_fail "expected FAIL-CLOSED abort message on stderr, not found"
        echo "----- stderr -----"; cat "$ERR"
    fi
    if grep -q "DRY RUN COMPLETE" "$OUT"; then
        note_fail "dry-run printed success line despite missing gitleaks (half-publish risk)"
    else
        note_ok "no false-success line emitted when gitleaks absent"
    fi
fi

echo ""
if [ "$fails" -ne 0 ]; then
    echo "=== $fails assertion(s) FAILED ==="
    exit 1
fi
echo "=== all publish-public self-tests passed ==="
exit 0
