#!/bin/sh
# Deterministic fake provider: apply the correct patch, then stall past the
# provider's local wall-clock timeout. The patch survives for acceptance, but
# the worker cannot reach a clean terminal state.
set -eu
state_dir="${FAKE_STATE_DIR:?FAKE_STATE_DIR must be set}"
count_file="$state_dir/count"
n=0
[ -f "$count_file" ] && n=$(cat "$count_file")
n=$((n + 1))
printf '%s\n' "$n" > "$count_file"
case "$n" in
  1)
    printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"replace_text","path":"src/lib.rs","expected":"for i in 1..n {","replacement":"for i in 1..=n {"},"usage":{"input_tokens":50,"output_tokens":5}}'
    ;;
  *)
    sleep 30
    printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"verify"},"usage":{"input_tokens":50,"output_tokens":5}}'
    ;;
esac
