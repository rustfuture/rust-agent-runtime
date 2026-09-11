#!/bin/sh
# Deterministic fake provider for the off_by_one fixture that produces
# *opposite* attempts:
#   attempt 1: correct source patch, then run_tool forever, so the worker ends
#              at the step limit while independent acceptance would pass;
#   attempt 2: worker-clean finish that weakens the visible test, so
#              independent acceptance fails.
# No single attempt satisfies both gates; the family must not pass.
#
# The harness gives every attempt a fresh <family>-att<N> workspace, which this
# script uses to select the attempt's scripted sequence.
set -eu
state_dir="${FAKE_STATE_DIR:?FAKE_STATE_DIR must be set}"
attempt="$(pwd)"
attempt="${attempt##*/}"
case "$attempt" in
  *-att1)
    count_file="$state_dir/attempt1-count"
    ;;
  *-att2)
    count_file="$state_dir/attempt2-count"
    ;;
  *)
    echo "opposite_attempts.sh: unexpected workspace $attempt" >&2
    exit 1
    ;;
esac
n=0
[ -f "$count_file" ] && n=$(cat "$count_file")
n=$((n + 1))
printf '%s\n' "$n" > "$count_file"
case "$attempt" in
  *-att1)
    case "$n" in
      1)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"read_file","path":"src/lib.rs"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
      2)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"replace_text","path":"src/lib.rs","expected":"for i in 1..n {","replacement":"for i in 1..=n {"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
      *)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"run_tool","program":"cargo","args":["test"]},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
    esac
    ;;
  *-att2)
    case "$n" in
      1)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"read_file","path":"tests/acceptance.rs"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
      2)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"replace_text","path":"tests/acceptance.rs","expected":"assert_eq!(sum_up_to(0), 0);\n    assert_eq!(sum_up_to(1), 1);\n    assert_eq!(sum_up_to(5), 15);","replacement":"assert_eq!(sum_up_to(0), 0);\n    assert_eq!(sum_up_to(1), 0);\n    assert_eq!(sum_up_to(5), 10);"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
      3)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"verify"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
      *)
        printf '%s\n' '{"status":"SUCCESS","structured_output":{"kind":"finish","summary":"test weakened"},"usage":{"input_tokens":50,"output_tokens":5}}'
        ;;
    esac
    ;;
esac
