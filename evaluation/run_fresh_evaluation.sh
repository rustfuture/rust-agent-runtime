#!/usr/bin/env bash
#
# Independent, fresh-fixture evaluation of the agent worker.
#
# For each defect family this script:
#   1. copies a pristine fixture and confirms its canonical acceptance test fails;
#   2. runs the durable worker (CLI `run`) on a separate agent copy with a
#      behavior-only task (no expected/replacement or solution code);
#   3. copies ONLY the agent's src/ over a fresh pristine crate and runs the
#      canonical test, so a model cannot succeed by editing or deleting tests.
#
# At most one controlled retry per family. All attempts are kept under
# evaluation/runs/<run_id>/.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FAMILIES=(off_by_one clamp_range prefix_format)
MODEL="${AGY_MODEL:-gemini-3.8-flash-low}"
AGY_BIN="${AGY_BIN:-agy}"
MAX_STEPS="${AGENT_MAX_STEPS:-6}"
TIMEOUT_SECS="${AGENT_TIMEOUT_SECS:-120}"

if ! command -v "$AGY_BIN" >/dev/null 2>&1; then
  echo "provider binary '$AGY_BIN' not found; cannot run the model evaluation" >&2
  exit 2
fi

RUN_ID="${RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT="$REPO_ROOT/evaluation/runs/$RUN_ID"
mkdir -p "$OUT"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agent-eval.XXXXXX")"
echo "run_id=$RUN_ID" | tee "$OUT/run.log"
echo "model=$MODEL" | tee -a "$OUT/run.log"
echo "work=$WORK" | tee -a "$OUT/run.log"

cargo build --locked --quiet
BIN="$REPO_ROOT/target/debug/rust-agent-runtime"

task_for() {
  case "$1" in
    off_by_one)
      echo "The crate in this workspace has a failing test: sum_up_to(n) is supposed to return the sum of the integers from 1 through n inclusive, but the current implementation returns the wrong total. Inspect the source, make the smallest correct change, and run cargo test until it passes."
      ;;
    clamp_range)
      echo "The crate in this workspace has a failing test: clamp(value, min, max) must return a value inside the inclusive range [min, max] and leave in-range values unchanged, but some inputs return a value outside the range. Inspect the source, make the smallest correct change, and run cargo test until it passes."
      ;;
    prefix_format)
      echo "The crate in this workspace has a failing test: label(prefix, name) is supposed to return the prefix followed by the trimmed name, but the current implementation drops the prefix. Inspect the source, make the smallest correct change, and run cargo test until it passes."
      ;;
  esac
}

mkdir -p "$WORK/pristine" "$WORK/agent" "$WORK/accept"

for family in "${FAMILIES[@]}"; do
  echo "== family: $family ==" | tee -a "$OUT/run.log"
  pristine="$WORK/pristine/$family"
  cp -R "$REPO_ROOT/evaluation/fresh_fixtures/$family" "$pristine"

  if (cd "$pristine" && cargo test --quiet) > "$OUT/$family-baseline.log" 2>&1; then
    echo "baseline unexpectedly passed for $family" | tee -a "$OUT/run.log"
    echo "baseline-passed" > "$OUT/$family.result"
    continue
  fi
  echo "baseline: acceptance test fails as expected"

  attempt=1
  pass=0
  while [[ $attempt -le 2 && $pass -eq 0 ]]; do
    agent_ws="$WORK/agent/$family-att$attempt"
    cp -R "$REPO_ROOT/evaluation/fresh_fixtures/$family" "$agent_ws"
    data_dir="$OUT/data/$family-att$attempt"
    log="$OUT/$family-attempt$attempt.log"
    echo "-- attempt $attempt --" | tee -a "$OUT/run.log"

    AGENT_TASK="$(task_for "$family")" \
    AGENT_ALLOWED="cargo" \
    AGENT_VERIFY="cargo test" \
    AGENT_MAX_STEPS="$MAX_STEPS" \
    AGENT_TIMEOUT_SECS="$TIMEOUT_SECS" \
    AGY_BIN="$AGY_BIN" \
    AGY_WORK_DIR="$agent_ws" \
    AGY_MODEL="$MODEL" \
      "$BIN" run "$data_dir" "$family-task" "$agent_ws" > "$log" 2>&1
    worker_status=$?

    accept="$WORK/accept/$family-att$attempt"
    cp -R "$pristine" "$accept"
    rm -rf "$accept/src"
    cp -R "$agent_ws/src" "$accept/src"
    accept_log="$OUT/$family-attempt$attempt-acceptance.log"
    if (cd "$accept" && cargo test --quiet) > "$accept_log" 2>&1; then
      pass=1
    else
      pass=0
    fi
    diff -u "$pristine/src/lib.rs" "$agent_ws/src/lib.rs" > "$OUT/$family-attempt$attempt.patch" 2>/dev/null || true
    echo "worker_status=$worker_status acceptance_pass=$pass" | tee -a "$OUT/run.log"

    attempt=$((attempt + 1))
  done

  if [[ $pass -eq 1 ]]; then
    echo "pass" > "$OUT/$family.result"
  else
    echo "fail" > "$OUT/$family.result"
  fi
  echo "$family: $(cat "$OUT/$family.result")" | tee -a "$OUT/run.log"
done

echo "== summary ==" | tee -a "$OUT/run.log"
for family in "${FAMILIES[@]}"; do
  printf '%s\t%s\n' "$family" "$(cat "$OUT/$family.result" 2>/dev/null || echo unknown)"
done | tee "$OUT/summary.tsv"

if [[ "${KEEP_WORK:-0}" != "1" ]]; then
  rm -rf "$WORK"
fi
echo "evidence: $OUT"
