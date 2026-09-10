#!/usr/bin/env bash
#
# Deterministic, no-real-model controls for evaluation/run_fresh_evaluation.sh.
#
# Each control drives the real harness with the scripted fake provider
# (evaluation/controls/fake_providers/*.sh) and asserts the recorded per-family
# results and exit code. No model call is made. The fake provider runs through
# the same supervised process path as the real provider, so timeout, output
# cap, process group, and cancellation bounds stay enabled, and tool programs
# still pass through the executor allowlist.
#
# Controls:
#   build failure .............. EVAL_CARGO_BIN fails; expect exit 2 + marker
#   invalid baseline ........... baseline passes; expect exit 2, not "expected"
#   acceptance failure ......... agent weakens the visible test; worker clean,
#                                independent acceptance fails
#   step limit ................. correct patch, no verify; acceptance passes,
#                                worker dirty with failure_kind=step_limit
#   wall timeout ............... correct patch, provider stalls; acceptance
#                                passes, worker dirty with failure_kind=wall_timeout
#   success .................... correct patch + verify + finish; exit 0
#   same RUN_ID rerun .......... second run is suffixed, first evidence stays
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$REPO_ROOT/evaluation/run_fresh_evaluation.sh"
CONTROLS="$REPO_ROOT/evaluation/controls"
RUNS="${EVAL_CONTROLS_RUNS:-$REPO_ROOT/evaluation/runs}"
STAMP="${EVAL_CONTROLS_STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE_ID="controls-$STAMP"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agent-eval-controls.XXXXXX")"
RESULTS="$CONTROLS/results.log"
: > "$RESULTS"

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

checks=0
failures=0
check() {
  description="$1"
  expected="$2"
  actual="$3"
  checks=$((checks + 1))
  if [ "$expected" = "$actual" ]; then
    echo "PASS $description (expected=$expected actual=$actual)" | tee -a "$RESULTS"
  else
    echo "FAIL $description (expected=$expected actual=$actual)" | tee -a "$RESULTS"
    failures=$((failures + 1))
  fi
}

LAST_STATUS=0
LAST_RUN=""
LAST_LOG=""
run_harness() {
  run_id="$1"
  fake="$2"
  family="$3"
  shift 3
  LAST_RUN="$run_id"
  LAST_LOG="$WORK/$run_id.out"
  state="$WORK/state-$run_id"
  rm -rf "$state"
  mkdir -p "$state"
  env RUN_ID="$run_id" \
    EVAL_FAKE_PROVIDER="$fake" \
    EVAL_FAMILIES="$family" \
    EVAL_MAX_ATTEMPTS=1 \
    FAKE_STATE_DIR="$state" \
    "$@" \
    bash "$HARNESS" > "$LAST_LOG" 2>&1
  LAST_STATUS=$?
  echo "== control $run_id exit=$LAST_STATUS" | tee -a "$RESULTS"
}

field() {
  awk -F= -v key="$2" '$1 == key { print $2 }' "$1" 2>/dev/null
}

present_if() {
  if [ -f "$1" ]; then
    echo present
  else
    echo missing
  fi
}

# 1. Build/configuration failure: no family may run, no stale binary may be used.
run_harness "$BASE_ID-build-failure" "$CONTROLS/fake_providers/success.sh" off_by_one \
  EVAL_CARGO_BIN="$CONTROLS/fake_cargo_fail.sh"
check "build failure exit code" 2 "$LAST_STATUS"
check "build failure marker" present "$(present_if "$RUNS/$LAST_RUN/build_failure")"
check "build failure did not write a family result" missing "$(present_if "$RUNS/$LAST_RUN/off_by_one.result")"

# 2. Invalid baseline: a baseline that unexpectedly passes must not be reported
#    as an expected failure and must not run the worker.
run_harness "$BASE_ID-invalid-baseline" "$CONTROLS/fake_providers/success.sh" passing_baseline \
  EVAL_FIXTURE_ROOT="$CONTROLS/fixtures"
check "invalid baseline exit code" 2 "$LAST_STATUS"
check "invalid baseline is not expected" false \
  "$(field "$RUNS/$LAST_RUN/passing_baseline.result" baseline_failed_as_expected)"

# 3. Acceptance failure: the worker cleanly completes by weakening the visible
#    test, but the independent acceptance copy catches it.
run_harness "$BASE_ID-acceptance-failure" "$CONTROLS/fake_providers/wrong_fix.sh" off_by_one
check "acceptance failure exit code" 1 "$LAST_STATUS"
check "acceptance failure worker clean" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" worker_clean_success)"
check "acceptance failure rejected" false \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" patch_acceptance_pass)"

# 4. Correct patch but no formal verify: run_tool cargo test does not clear the
#    debt, so the worker stops at the step limit while acceptance still passes.
run_harness "$BASE_ID-step-limit" "$CONTROLS/fake_providers/step_limit.sh" off_by_one \
  AGENT_MAX_STEPS=4
check "step limit exit code" 1 "$LAST_STATUS"
check "step limit acceptance passes" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" patch_acceptance_pass)"
check "step limit worker not clean" false \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" worker_clean_success)"
check "step limit failure kind" step_limit \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" failure_kind)"

# 5. Correct patch but provider wall-clock timeout.
run_harness "$BASE_ID-wall-timeout" "$CONTROLS/fake_providers/wall_timeout.sh" off_by_one \
  AGENT_TIMEOUT_SECS=2 AGENT_MAX_STEPS=3
check "wall timeout exit code" 1 "$LAST_STATUS"
check "wall timeout acceptance passes" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" patch_acceptance_pass)"
check "wall timeout worker not clean" false \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" worker_clean_success)"
check "wall timeout failure kind" wall_timeout \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" failure_kind)"

# 6. Fully successful run.
run_harness "$BASE_ID-success" "$CONTROLS/fake_providers/success.sh" off_by_one
check "success exit code" 0 "$LAST_STATUS"
check "success acceptance passes" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" patch_acceptance_pass)"
check "success worker clean" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" worker_clean_success)"
check "success failure kind" none \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" failure_kind)"

# 7. One family failing (missing fixture) must not stop a later family from
#    running and being recorded.
run_harness "$BASE_ID-mixed-families" "$CONTROLS/fake_providers/success.sh" "missing_family off_by_one"
check "mixed families exit code" 2 "$LAST_STATUS"
check "mixed families later family still runs" present \
  "$(present_if "$RUNS/$LAST_RUN/missing_family.result")"
check "mixed families healthy family passes" true \
  "$(field "$RUNS/$LAST_RUN/off_by_one.result" worker_clean_success)"
rows=$(awk 'END { print NR - 1 }' "$RUNS/$LAST_RUN/summary.tsv" 2>/dev/null || echo 0)
check "mixed families both recorded" 2 "$rows"

# 8. Same RUN_ID rerun: the second run must not overwrite the first evidence.
run_harness "$BASE_ID-rerun" "$CONTROLS/fake_providers/success.sh" off_by_one
first_status=$LAST_STATUS
first_dir="$RUNS/$BASE_ID-rerun"
run_harness "$BASE_ID-rerun" "$CONTROLS/fake_providers/success.sh" off_by_one
second_status=$LAST_STATUS
check "same-run-id first exit code" 0 "$first_status"
check "same-run-id second exit code" 0 "$second_status"
check "same-run-id first evidence preserved" present "$(present_if "$first_dir/summary.tsv")"
check "same-run-id second run suffixed" present "$(present_if "$first_dir-1/summary.tsv")"

echo "controls: $checks checks, $failures failures" | tee -a "$RESULTS"
if [ "$failures" -gt 0 ]; then
  exit 1
fi
exit 0
