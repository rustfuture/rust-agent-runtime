#!/usr/bin/env bash
#
# Independent, fresh-fixture evaluation of the agent worker.
#
# For each defect family this script:
#   1. builds the runtime (a build failure stops the run; a stale binary is
#      never used);
#   2. copies a pristine fixture and confirms its canonical acceptance test
#      fails (a build/dependency error there is NOT an expected baseline
#      failure);
#   3. runs the durable worker (CLI `run`) on a separate agent copy with a
#      behavior-only task (no expected/replacement or solution code);
#   4. copies ONLY the agent's src/ over a fresh pristine crate and runs the
#      canonical test, so a model cannot succeed by editing or deleting tests.
#
# Per-family results are recorded separately:
#   baseline_failed_as_expected, harness_ok, patch_acceptance_pass,
#   worker_clean_success, failure_kind (step_limit | wall_timeout | cancelled |
#   provider_error | other | none). Those fields always describe one single
#   attempt, never a combination across attempts; per-attempt rows are kept in
#   <family>-attempts.tsv.
#
# Acceptance policy for the process exit code:
#   0  every family passed baseline, harness, independent acceptance, and a
#      clean worker terminal state within the same attempt;
#   2  a harness/build failure occurred for any family (build, copy, config,
#      evidence write) even if other families ran;
#   1  the harness was mechanically sound but at least one family did not fully
#      pass.
#
# At most one controlled retry per family. Every attempt is kept under
# evaluation/runs/<run_id>/. A RUN_ID that already exists is suffixed with a
# counter instead of overwriting evidence.
#
# Deterministic, no-real-model controls run this script with a fake provider
# and control fixtures (see evaluation/run_controls.sh):
#   EVAL_FAKE_PROVIDER  script emitting AGY-shaped JSON (same supervised path,
#                       so timeout/output/cancel/allowlist bounds stay on)
#   EVAL_FIXTURE_ROOT   fixture directory (default evaluation/fresh_fixtures)
#   EVAL_FAMILIES       space-separated family list
#   EVAL_CARGO_BIN      cargo replacement for harness build/test commands
#   EVAL_MAX_ATTEMPTS   attempts per family (default 2)
#   EVAL_AGENT_ALLOWED  AGENT_ALLOWED (default cargo)
#   EVAL_AGENT_VERIFY   AGENT_VERIFY (default cargo test)
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FAMILIES_ENV="${EVAL_FAMILIES:-off_by_one clamp_range prefix_format}"
read -r -a FAMILIES <<< "$FAMILIES_ENV"
FIXTURE_ROOT="${EVAL_FIXTURE_ROOT:-$REPO_ROOT/evaluation/fresh_fixtures}"
CARGO_BIN="${EVAL_CARGO_BIN:-cargo}"
MODEL="${AGY_MODEL:-gemini-3.8-flash-low}"
AGY_BIN="${AGY_BIN:-agy}"
MAX_STEPS="${AGENT_MAX_STEPS:-6}"
TIMEOUT_SECS="${AGENT_TIMEOUT_SECS:-120}"
MAX_ATTEMPTS="${EVAL_MAX_ATTEMPTS:-2}"
FAKE_PROVIDER="${EVAL_FAKE_PROVIDER:-}"
AGENT_ALLOWED="${EVAL_AGENT_ALLOWED:-cargo}"
AGENT_VERIFY="${EVAL_AGENT_VERIFY:-cargo test}"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# Capture the pre-run source state before this harness creates any evidence
# directory, so the dirty count cannot include its own run output.
SOURCE_SHA="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY_COUNT="$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"

RUN_ID="${RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE_OUT="$REPO_ROOT/evaluation/runs/$RUN_ID"
OUT="$BASE_OUT"
collision=0
while [ -e "$OUT" ]; do
  collision=$((collision + 1))
  if [ "$collision" -ge 100 ]; then
    echo "run_id $RUN_ID collides with too many existing runs; refusing to continue" >&2
    exit 2
  fi
  OUT="${BASE_OUT}-${collision}"
done
mkdir -p "$OUT" || exit 2

WORK="$(mktemp -d "${TMPDIR:-/tmp}/agent-eval.XXXXXX")" || exit 2
mkdir -p "$WORK/pristine" "$WORK/agent" "$WORK/accept" || {
  echo "failed to create harness work directories under $WORK" >&2
  exit 2
}
cleanup() {
  if [ "${KEEP_WORK:-0}" != "1" ]; then
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

run_log="$OUT/run.log"
: > "$run_log"
{
  echo "run_id=$(basename "$OUT")"
  echo "requested_run_id=$RUN_ID"
  echo "run_id_collisions=$collision"
  echo "model=$MODEL"
  echo "provider_binary=$AGY_BIN"
  echo "fake_provider=${FAKE_PROVIDER:-none}"
  echo "fixture_root=$FIXTURE_ROOT"
  echo "families=${FAMILIES[*]}"
  echo "max_steps=$MAX_STEPS"
  echo "timeout_secs=$TIMEOUT_SECS"
  echo "max_attempts=$MAX_ATTEMPTS"
  echo "agent_allowed=$AGENT_ALLOWED"
  echo "agent_verify=$AGENT_VERIFY"
  echo "work=$WORK"
} | tee -a "$run_log" >/dev/null

harness_failed=false
fail_harness() {
  harness_failed=true
  echo "harness_failure: $*" | tee -a "$run_log" >&2
}

if [ -z "$FAKE_PROVIDER" ] && ! command -v "$AGY_BIN" >/dev/null 2>&1; then
  echo "provider binary '$AGY_BIN' not found; cannot run the model evaluation" | tee -a "$run_log" >&2
  echo "build_failure" > "$OUT/harness_failure"
  exit 2
fi

# Build first. A failed or missing binary must never fall through to a stale
# artifact from an earlier run.
build_cmd=("$CARGO_BIN" build --locked --quiet)
if ! "${build_cmd[@]}" > "$OUT/build.log" 2>&1; then
  echo "build_failure: ${build_cmd[*]} failed (see build.log)" | tee -a "$run_log" >&2
  echo "build_failure" > "$OUT/build_failure"
  exit 2
fi
BIN="$REPO_ROOT/target/debug/rust-agent-runtime"
if [ ! -x "$BIN" ]; then
  echo "build_failure: $BIN is missing after a successful build" | tee -a "$run_log" >&2
  echo "build_failure" > "$OUT/harness_failure"
  exit 2
fi
BIN_SHA="$(sha256 "$BIN")"
HARNESS_SHA="$(sha256 "$REPO_ROOT/evaluation/run_fresh_evaluation.sh")"
PROVIDER_VERSION="$("$AGY_BIN" --version 2>/dev/null | head -n 1 || true)"
{
  echo "built_binary=$BIN"
  echo "built_binary_sha256=$BIN_SHA"
  echo "source_sha=$SOURCE_SHA"
  echo "source_dirty_count=$DIRTY_COUNT"
  echo "harness_sha256=$HARNESS_SHA"
  echo "provider_version=${PROVIDER_VERSION:-unavailable}"
  echo "build_command=${build_cmd[*]}"
  echo "worker_command_template=AGENT_TASK=<task> AGENT_ALLOWED=$AGENT_ALLOWED AGENT_VERIFY=$AGENT_VERIFY AGENT_MAX_STEPS=$MAX_STEPS AGENT_TIMEOUT_SECS=$TIMEOUT_SECS AGY_BIN=$AGY_BIN AGY_MODEL=$MODEL AGY_WORK_DIR=<workspace> [AGENT_FAKE_PROVIDER=<script>] $BIN run <data_dir> <task_id> <workspace>"
  if [ "$collision" != "0" ]; then
    echo "warning=run_id_collision_suffixed_$collision"
  fi
} > "$OUT/metadata.txt"

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
    *)
      echo "The crate in this workspace has a failing test. Inspect the source, make the smallest correct change, and run cargo test until it passes."
      ;;
  esac
}

printf 'family\tbaseline_failed_as_expected\tharness_ok\tpatch_acceptance_pass\tworker_clean_success\tfailure_kind\tattempts\tworker_seconds\tinput_tokens\toutput_tokens\n' > "$OUT/summary.tsv"

all_pass=true
family_count=${#FAMILIES[@]}
index=0
while [ "$index" -lt "$family_count" ]; do
  family="${FAMILIES[$index]}"
  index=$((index + 1))
  fixture="$FIXTURE_ROOT/$family"
  pristine="$WORK/pristine/$family"
  baseline=missing_fixture
  harness_ok=true
  family_pass=false
  family_acceptance=false
  family_clean=false
  family_failure_kind=none
  attempts_acceptance_pass=0
  attempts_worker_clean=0
  family_attempts=0
  worker_seconds=0
  input_tokens=0
  output_tokens=0

  echo "== family: $family ==" | tee -a "$run_log" >/dev/null

  if [ ! -d "$fixture" ]; then
    fail_harness "fixture $fixture does not exist for family $family"
    harness_ok=false
    family_failure_kind=other
  else
    if ! cp -R "$fixture" "$pristine" > "$OUT/$family-pristine-copy.log" 2>&1; then
      fail_harness "pristine fixture copy failed for $family"
      harness_ok=false
      baseline=copy_error
      family_failure_kind=other
    fi
  fi

  if [ "$harness_ok" = true ]; then
    baseline_build_log="$OUT/$family-baseline-build.log"
    baseline_log="$OUT/$family-baseline.log"
    if ! (cd "$pristine" && "$CARGO_BIN" test --quiet --no-run) > "$baseline_build_log" 2>&1; then
      # A crate that does not build is a harness/configuration problem, not an
      # expected baseline test failure.
      fail_harness "baseline build failed for $family (see $(basename "$baseline_build_log"))"
      harness_ok=false
      baseline=build_error
      family_failure_kind=other
    elif (cd "$pristine" && "$CARGO_BIN" test --quiet) > "$baseline_log" 2>&1; then
      fail_harness "baseline unexpectedly passed for $family"
      baseline=unexpected_pass
      family_failure_kind=other
    else
      baseline=failed_as_expected
    fi
  fi

  attempt=1
  while [ "$harness_ok" = true ] \
    && [ "$baseline" = "failed_as_expected" ] \
    && [ "$attempt" -le "$MAX_ATTEMPTS" ] \
    && [ "$family_pass" != true ]; do
    family_attempts=$((family_attempts + 1))
    agent_ws="$WORK/agent/$family-att$attempt"
    data_dir="$OUT/data/$family-att$attempt"
    log="$OUT/$family-attempt$attempt.log"
    echo "-- attempt $attempt --" | tee -a "$run_log" >/dev/null

    rm -rf "$agent_ws"
    if ! cp -R "$fixture" "$agent_ws" > "$OUT/$family-attempt$attempt-copy.log" 2>&1; then
      fail_harness "agent workspace copy failed for $family attempt $attempt"
      harness_ok=false
      family_failure_kind=other
      break
    fi
    mkdir -p "$data_dir"

    task="$(task_for "$family")"
    started_s=$(date +%s)
    status_file="$WORK/$family-att$attempt.status"
    rm -f "$status_file"
    (
      env AGENT_TASK="$task" \
        AGENT_ALLOWED="$AGENT_ALLOWED" \
        AGENT_VERIFY="$AGENT_VERIFY" \
        AGENT_MAX_STEPS="$MAX_STEPS" \
        AGENT_TIMEOUT_SECS="$TIMEOUT_SECS" \
        AGY_BIN="$AGY_BIN" \
        AGY_WORK_DIR="$agent_ws" \
        AGY_MODEL="$MODEL" \
        AGENT_FAKE_PROVIDER="$FAKE_PROVIDER" \
        "$BIN" run "$data_dir" "$family-task" "$agent_ws" > "$log" 2>&1
      echo $? > "$status_file"
    ) &
    worker_pid=$!
    worker_timed_out=false
    wall_limit=$((TIMEOUT_SECS * (MAX_STEPS + 2) + 60))
    while [ ! -f "$status_file" ]; do
      sleep 1
      if [ $(( $(date +%s) - started_s )) -ge "$wall_limit" ]; then
        worker_timed_out=true
        kill -9 "$worker_pid" 2>/dev/null
        break
      fi
    done
    wait "$worker_pid" 2>/dev/null
    if [ -f "$status_file" ]; then
      worker_status="$(cat "$status_file")"
    else
      worker_status=124
    fi
    worker_seconds=$((worker_seconds + $(date +%s) - started_s))

    state="$(awk -F'\t' -v id="$family-task" '$1 == id { value = $2 } END { print value }' "$data_dir/events.log" 2>/dev/null || true)"
    [ -n "$state" ] || state=unknown
    worker_clean=false
    if [ "$worker_status" = "0" ] && [ "$state" = "succeeded" ]; then
      worker_clean=true
    fi

    attempt_input="$(grep -m1 '^input_tokens=' "$log" 2>/dev/null | cut -d= -f2 || true)"
    attempt_output="$(grep -m1 '^output_tokens=' "$log" 2>/dev/null | cut -d= -f2 || true)"
    case "$attempt_input" in ''|*[!0-9]*) attempt_input=0 ;; esac
    case "$attempt_output" in ''|*[!0-9]*) attempt_output=0 ;; esac
    input_tokens=$((input_tokens + attempt_input))
    output_tokens=$((output_tokens + attempt_output))

    failure_kind=none
    if [ "$worker_clean" != true ]; then
      if [ "$worker_timed_out" = true ]; then
        failure_kind=wall_timeout
      elif grep -q "agent step limit reached" "$log" 2>/dev/null; then
        failure_kind=step_limit
      elif grep -q "provider call exceeded the local timeout" "$log" 2>/dev/null; then
        failure_kind=wall_timeout
      elif grep -q "provider stream exceeded the local timeout" "$log" 2>/dev/null; then
        failure_kind=wall_timeout
      elif grep -q "cancelled" "$log" 2>/dev/null; then
        failure_kind=cancelled
      elif grep -Eq "AGY failed with status|AGY status was|structured_output|invalid type|missing field|failed to execute" "$log" 2>/dev/null; then
        failure_kind=provider_error
      else
        failure_kind=other
      fi
    fi

    acceptance=false
    accept="$WORK/accept/$family-att$attempt"
    accept_copy_log="$OUT/$family-attempt$attempt-acceptance-copy.log"
    rm -rf "$accept"
    if cp -R "$pristine" "$accept" > "$accept_copy_log" 2>&1 \
      && rm -rf "$accept/src" >> "$accept_copy_log" 2>&1 \
      && cp -R "$agent_ws/src" "$accept/src" >> "$accept_copy_log" 2>&1 \
      && [ -f "$accept/src/lib.rs" ] \
      && cmp -s "$accept/src/lib.rs" "$agent_ws/src/lib.rs"; then
      if (cd "$accept" && "$CARGO_BIN" test --quiet) > "$OUT/$family-attempt$attempt-acceptance.log" 2>&1; then
        acceptance=true
      fi
    else
      fail_harness "acceptance copy failed for $family attempt $attempt; acceptance was not run"
      harness_ok=false
      failure_kind=other
    fi
    # One attempt is one record. The recorded fields describe the passing
    # attempt if there is one, otherwise the latest attempt tried; acceptance
    # from one attempt is never latched onto a clean worker from another.
    if [ "$acceptance" = true ]; then
      attempts_acceptance_pass=$((attempts_acceptance_pass + 1))
    fi
    if [ "$worker_clean" = true ]; then
      attempts_worker_clean=$((attempts_worker_clean + 1))
    fi
    family_acceptance="$acceptance"
    family_clean="$worker_clean"
    family_failure_kind="$failure_kind"
    if [ "$harness_ok" = true ] \
      && [ "$baseline" = "failed_as_expected" ] \
      && [ "$acceptance" = true ] \
      && [ "$worker_clean" = true ] \
      && [ "$failure_kind" = "none" ]; then
      family_pass=true
    fi

    diff -ruN --exclude=target "$pristine" "$agent_ws" > "$OUT/$family-attempt$attempt.patch" 2>/dev/null || true
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$family" "$attempt" "$worker_status" "$state" "$worker_clean" "$acceptance" \
      "$failure_kind" "$(( $(date +%s) - started_s ))" "$attempt_input" \
      >> "$OUT/$family-attempts.tsv"
    echo "worker_status=$worker_status state=$state worker_clean=$worker_clean acceptance_pass=$acceptance failure_kind=$failure_kind" | tee -a "$run_log" >/dev/null

    attempt=$((attempt + 1))
  done

  if [ "$baseline" != "failed_as_expected" ] || [ "$harness_ok" != true ]; then
    family_failure_kind=${family_failure_kind:-other}
    family_acceptance=false
    family_clean=false
    family_pass=false
  fi
  pass="$family_pass"
  if [ "$pass" != true ]; then
    all_pass=false
  fi

  baseline_ok=false
  [ "$baseline" = "failed_as_expected" ] && baseline_ok=true
  {
    echo "family=$family"
    echo "family_pass=$pass"
    echo "baseline=$baseline"
    echo "baseline_failed_as_expected=$baseline_ok"
    echo "harness_ok=$harness_ok"
    echo "patch_acceptance_pass=$family_acceptance"
    echo "worker_clean_success=$family_clean"
    echo "failure_kind=$family_failure_kind"
    echo "attempts=$family_attempts"
    echo "attempts_acceptance_pass=$attempts_acceptance_pass"
    echo "attempts_worker_clean=$attempts_worker_clean"
    echo "worker_seconds=$worker_seconds"
    echo "input_tokens=$input_tokens"
    echo "output_tokens=$output_tokens"
  } > "$OUT/$family.result"

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$family" "$baseline_ok" "$harness_ok" "$family_acceptance" "$family_clean" \
    "$family_failure_kind" "$family_attempts" "$worker_seconds" \
    "$input_tokens" "$output_tokens" >> "$OUT/summary.tsv"
  echo "$family: pass=$pass baseline=$baseline harness_ok=$harness_ok acceptance=$family_acceptance worker_clean=$family_clean failure_kind=$family_failure_kind" | tee -a "$run_log" >/dev/null
done

if [ "$harness_failed" = true ]; then
  exit_status=2
elif [ "$all_pass" = true ]; then
  exit_status=0
else
  exit_status=1
fi
echo "summary:" | tee -a "$run_log" >/dev/null
cat "$OUT/summary.tsv" >> "$run_log"
echo "exit_status=$exit_status" | tee -a "$run_log" >/dev/null
echo "evidence: $OUT"
exit "$exit_status"
