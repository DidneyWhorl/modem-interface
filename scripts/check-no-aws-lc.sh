#!/bin/sh
# Fails if aws-lc-sys re-enters the router build graph (a dep bump can silently
# re-arm rustls's default aws_lc_rs provider). See spec §5.1.
set -e
cd "$(dirname "$0")/../backend"

# Capture stdout+stderr and the exit code so we can distinguish three outcomes:
#   1. cargo tree fails with "did not match any packages" -> genuine absence, pass
#   2. cargo tree succeeds and lists aws-lc-sys            -> regression, fail
#   3. cargo tree fails for any other reason               -> can't verify, fail loudly
set +e
output=$(cargo tree -i aws-lc-sys --no-default-features --features real-hardware,tls,embedded-frontend 2>&1)
status=$?
set -e

if [ "$status" -ne 0 ]; then
  case "$output" in
    *"did not match any packages"*)
      echo "OK: aws-lc-sys absent from the router build graph."
      exit 0
      ;;
    *)
      echo "ERROR: cargo tree failed (exit $status); cannot verify aws-lc-sys absence:" >&2
      echo "$output" >&2
      exit "$status"
      ;;
  esac
fi

if echo "$output" | grep -q aws-lc-sys; then
  echo "ERROR: aws-lc-sys is back in the router build graph (the mipsel blocker). Re-check rustls consumer features." >&2
  exit 1
fi

# cargo tree exited 0 without naming the package — unexpected, but it is absent.
echo "OK: aws-lc-sys absent from the router build graph."
