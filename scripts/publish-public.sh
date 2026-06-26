#!/bin/sh
# publish-public.sh — allowlist-driven public-mirror publisher (gate (d) Phase C).
#
# Exports ONLY the allowlisted public surface from a clean git ref into a fresh
# staging tree, applies a defensive URL-substitution transform, runs gitleaks
# fail-closed, then (non-dry-run) stages into a clean checkout of the public
# repo and creates a LOCAL "Release vX.Y.Z" commit + tag. It STOPS before any
# `git push`. This script NEVER pushes.
#
# Spec: docs/superpowers/specs/2026-06-19-repo-split-public-mirror-design.md §3/§4/§5
#
# Usage:
#   publish-public.sh [--dry-run|--prepare-repo] <git-ref> [public-repo-dir]
#
#   <git-ref>          release tag/ref to export, e.g. v1.4.0 (or $PUBLISH_REF)
#   [public-repo-dir]  clean checkout of the public GitHub repo (only used in
#                      --prepare-repo mode; or $PUBLIC_REPO_DIR)
#
#   --dry-run      (DEFAULT) export + substitute + manifest print + scan; does
#                  NOT need the public repo cloned. Safe to run right now.
#   --prepare-repo export + substitute + scan, THEN stage into the public repo
#                  checkout and create the local commit + tag. Still no push.
#
# Exit codes:
#   0  success (dry-run completed, or repo prepared up to the no-push stop)
#   1  a publish-blocking failure (scan finding, gitleaks absent, etc.)
#   2  usage / argument error
#
# Safety properties (spec §5): allowlist not denylist; defensive transform;
# gitleaks fail-closed; manual-review stop before any push.
set -eu

# ---- constants -----------------------------------------------------------
PUBLIC_REPO_URL="https://github.com/DidneyWhorl/modem-interface"
PUBLIC_FEED_HOST="packages.ctrl-modem.com"
PRIV_GIT_HOST="github.com/DidneyWhorl/modem-interface"
PRIV_FEED_HOST="packages.ctrl-modem.com"

# Resolve repo root from this script's location (scripts/ is one level down).
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$SCRIPT_DIR/public-manifest.txt"
# gitleaks config for the scan gate. Extends the default ruleset and adds ONE
# surgical allowlist for the env_cmd.rs localhost TLS test fixture (see the
# file's header for the why). This config lives in the publishing tooling and is
# NOT in public-manifest.txt, so it never enters the published surface.
GITLEAKS_CONFIG="$SCRIPT_DIR/gitleaks-public.toml"

usage() {
    echo "usage: $0 [--dry-run|--prepare-repo] <git-ref> [public-repo-dir]" >&2
    exit 2
}

# ---- arg parsing ---------------------------------------------------------
MODE="dry-run"
REF="${PUBLISH_REF:-}"
PUBLIC_REPO_DIR="${PUBLIC_REPO_DIR:-}"

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)       MODE="dry-run" ;;
        --prepare-repo)  MODE="prepare-repo" ;;
        -h|--help)       usage ;;
        -*)              echo "error: unknown flag: $1" >&2; usage ;;
        *)
            if [ -z "$REF" ]; then
                REF="$1"
            elif [ -z "$PUBLIC_REPO_DIR" ]; then
                PUBLIC_REPO_DIR="$1"
            else
                echo "error: too many positional args: $1" >&2; usage
            fi
            ;;
    esac
    shift
done

[ -n "$REF" ] || { echo "error: no <git-ref> given" >&2; usage; }
[ -f "$MANIFEST" ] || { echo "error: manifest not found: $MANIFEST" >&2; exit 2; }
[ -f "$GITLEAKS_CONFIG" ] || { echo "error: gitleaks config not found: $GITLEAKS_CONFIG" >&2; exit 2; }

# Validate the ref resolves to a commit in THIS repo.
if ! git -C "$REPO_ROOT" rev-parse --verify --quiet "${REF}^{commit}" >/dev/null; then
    echo "error: git ref does not resolve to a commit: $REF" >&2
    exit 2
fi

# ---- temp dirs + cleanup trap -------------------------------------------
WORKTREE=""
STAGE=""
cleanup() {
    # Remove the detached worktree via git so its admin entry is also pruned.
    if [ -n "$WORKTREE" ] && [ -d "$WORKTREE" ]; then
        git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" 2>/dev/null || rm -rf "$WORKTREE"
    fi
    [ -n "$STAGE" ] && [ -d "$STAGE" ] && rm -rf "$STAGE"
    git -C "$REPO_ROOT" worktree prune 2>/dev/null || true
}
trap cleanup EXIT INT TERM

WORKTREE="$(mktemp -d)"
STAGE="$(mktemp -d)"

echo "=== publish-public.sh (mode: $MODE) ==="
echo "ref:          $REF"
echo "repo root:    $REPO_ROOT"
echo "manifest:     $MANIFEST"
echo "worktree:     $WORKTREE"
echo "staging:      $STAGE"
echo ""

# ---- step 1: clean export of allowlisted paths ---------------------------
# A detached worktree gives us the ref's tree with ORIGINAL line endings (no
# git-archive eol-attribute CRLF mangling). We then copy only allowlisted paths.
echo "--- step 1: clean export (worktree + allowlist copy) ---"
# Reuse the worktree dir mktemp created (git worktree wants a non-existent path).
rmdir "$WORKTREE"
git -C "$REPO_ROOT" worktree add --quiet --detach "$WORKTREE" "$REF"

# Read manifest into INCLUDE and EXCLUDE lists.
copy_one() {
    # $1 = path entry (already glob-expanded by caller); copy from worktree->stage
    src="$WORKTREE/$1"
    dst="$STAGE/$1"
    if [ -d "$src" ]; then
        mkdir -p "$dst"
        # cp -R the dir contents preserving structure.
        cp -R "$src/." "$dst/"
    elif [ -e "$src" ]; then
        mkdir -p "$(dirname "$dst")"
        cp "$src" "$dst"
    else
        echo "  warn: include not present in ref, skipping: $1" >&2
        return 1
    fi
    return 0
}

apply_exclude() {
    # $1 = path entry to remove from the staging tree.
    rm -rf "$STAGE/$1"
}

# First pass: includes. Second pass: excludes (carve-outs).
INCLUDED=0
while IFS= read -r raw || [ -n "$raw" ]; do
    # strip leading/trailing whitespace
    entry="$(printf '%s' "$raw" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    [ -z "$entry" ] && continue
    case "$entry" in
        \#*) continue ;;     # comment
        !*)  continue ;;     # exclude — handled in pass 2
    esac
    # Glob-expand relative to the worktree so build-* style entries work.
    matched=0
    for g in $(cd "$WORKTREE" && ls -d $entry 2>/dev/null || true); do
        if copy_one "$g"; then matched=1; INCLUDED=$((INCLUDED+1)); fi
    done
    if [ "$matched" -eq 0 ]; then
        # No glob match — try literal (copy_one already warns if absent).
        copy_one "$entry" || true
    fi
done < "$MANIFEST"

# Pass 2: excludes.
while IFS= read -r raw || [ -n "$raw" ]; do
    entry="$(printf '%s' "$raw" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    case "$entry" in
        !*) apply_exclude "${entry#!}"; echo "  carve-out: removed ${entry#!}" ;;
        *)  : ;;
    esac
done < "$MANIFEST"

echo "  staged $INCLUDED top-level include match(es)."
echo ""

# ---- step 2: defensive URL-substitution transform ------------------------
# Belt-and-suspenders vs Phase B: rewrite any residual private-infra host.
echo "--- step 2: defensive URL substitution ---"
SUBST_COUNT=0
# Find regular files in the staging tree and rewrite in place (portable sed).
# Order matters: rewrite git host and feed host independently.
find "$STAGE" -type f | while IFS= read -r f; do
    if grep -q -e "$PRIV_GIT_HOST" -e "$PRIV_FEED_HOST" "$f" 2>/dev/null; then
        # artifacts.* first (more specific), then git.*; escape dots for sed.
        sed -i \
            -e "s#${PRIV_FEED_HOST}#${PUBLIC_FEED_HOST}#g" \
            -e "s#${PRIV_GIT_HOST}#${PUBLIC_REPO_URL#https://}#g" \
            "$f"
        echo "  rewrote: ${f#$STAGE/}"
    fi
done
# Re-scan to report whether any residual private host survived the transform.
RESIDUAL="$(grep -rIl -e "$PRIV_GIT_HOST" -e "$PRIV_FEED_HOST" "$STAGE" 2>/dev/null || true)"
if [ -n "$RESIDUAL" ]; then
    echo "  WARNING: private-infra host still present after substitution in:" >&2
    printf '    %s\n' $RESIDUAL >&2
else
    echo "  no residual github.com/DidneyWhorl/modem-interface / packages.ctrl-modem.com host markers."
fi
echo ""

# ---- staged manifest print (ALWAYS, before the scan gate) ----------------
# Printed before step 3 so reviewers (and the self-test) see the exact file set
# that would be published EVEN when the fail-closed scan aborts the run.
echo "=== STAGED MANIFEST (files that would be published) ==="
( cd "$STAGE" && find . -type f | sed 's#^\./##' | LC_ALL=C sort )
STAGED_TOTAL="$(find "$STAGE" -type f | wc -l | tr -d '[:space:]')"
echo "--- total staged files: $STAGED_TOTAL ---"
echo ""

# ---- step 3: secret scan (gitleaks) — FAIL-CLOSED ------------------------
echo "--- step 3: secret scan (gitleaks, fail-closed) ---"
SCAN_REPORT="$STAGE.gitleaks-report.json"
if ! command -v gitleaks >/dev/null 2>&1; then
    echo "ERROR: gitleaks is not installed. FAIL-CLOSED: aborting publish." >&2
    echo "       Install gitleaks before the real publish (Phase D)." >&2
    echo "       The secret scan is a mandatory gate (spec §5.3) and is NEVER" >&2
    echo "       skipped. No staged tree is published without a passing scan." >&2
    exit 1
fi

# gitleaks detect: exit 0 = no leaks, non-zero = findings or error. Either way,
# a non-zero result means we do NOT proceed.
if gitleaks detect \
        --source "$STAGE" \
        --no-git \
        --redact \
        --config "$GITLEAKS_CONFIG" \
        --report-format json \
        --report-path "$SCAN_REPORT" \
        --exit-code 1; then
    echo "  gitleaks: clean (no findings)."
else
    rc=$?
    echo "ERROR: gitleaks reported findings or errored (exit $rc)." >&2
    echo "       FAIL-CLOSED: aborting publish. Review the report:" >&2
    echo "       $SCAN_REPORT" >&2
    [ -f "$SCAN_REPORT" ] && cat "$SCAN_REPORT" >&2 || true
    exit 1
fi
echo ""

# ---- derive version from the exported Cargo.toml -------------------------
CARGO_TOML="$STAGE/backend/Cargo.toml"
if [ ! -f "$CARGO_TOML" ]; then
    echo "error: backend/Cargo.toml not in staged tree; cannot derive version" >&2
    exit 1
fi
VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$CARGO_TOML" | head -n1)"
[ -n "$VERSION" ] || { echo "error: could not parse version from Cargo.toml" >&2; exit 1; }
TAGNAME="v$VERSION"
echo "--- derived version: $VERSION  (tag: $TAGNAME) ---"
echo ""

# ---- step 4: (prepare-repo) stage into public repo + local commit/tag ----
if [ "$MODE" = "dry-run" ]; then
    echo "=== DRY RUN COMPLETE — nothing was committed, nothing was pushed. ==="
    echo "Re-run with --prepare-repo <ref> <public-repo-dir> to build the local"
    echo "commit + tag (still NO push)."
    exit 0
fi

# prepare-repo mode requires a clean public repo checkout.
[ -n "$PUBLIC_REPO_DIR" ] || { echo "error: --prepare-repo needs <public-repo-dir>" >&2; usage; }
[ -d "$PUBLIC_REPO_DIR/.git" ] || { echo "error: not a git repo: $PUBLIC_REPO_DIR" >&2; exit 2; }

# Require a clean working tree so we never mix with pre-existing junk.
if [ -n "$(git -C "$PUBLIC_REPO_DIR" status --porcelain)" ]; then
    echo "error: public repo working tree is not clean: $PUBLIC_REPO_DIR" >&2
    echo "       refusing to stage into a dirty checkout." >&2
    exit 1
fi

echo "--- step 4: stage into public repo + local commit/tag ---"
# Wipe tracked content (clean working tree), then drop in the staged surface.
# Keep .git only.
find "$PUBLIC_REPO_DIR" -mindepth 1 -maxdepth 1 ! -name '.git' -exec rm -rf {} +
cp -R "$STAGE/." "$PUBLIC_REPO_DIR/"

git -C "$PUBLIC_REPO_DIR" add -A
git -C "$PUBLIC_REPO_DIR" commit -q -m "Release $TAGNAME"
# Annotated tag mirrors the private release tag style.
git -C "$PUBLIC_REPO_DIR" tag -a "$TAGNAME" -m "Release $TAGNAME"

echo "  created local commit 'Release $TAGNAME' + tag '$TAGNAME' in:"
echo "    $PUBLIC_REPO_DIR"
echo ""
echo "=== STOP: no push performed. ==="
echo "Manual review checklist before pushing (Phase D, with Richard's go):"
echo "  1. Inspect: git -C \"$PUBLIC_REPO_DIR\" show --stat $TAGNAME"
echo "  2. Re-read the STAGED MANIFEST printed above."
echo "  3. Confirm the gitleaks report was clean."
echo "  4. Then, and only then: git -C \"$PUBLIC_REPO_DIR\" push origin HEAD $TAGNAME"
echo "This script did NOT push and never will."
exit 0
