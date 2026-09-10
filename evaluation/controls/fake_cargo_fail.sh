#!/bin/sh
# Deterministic EVAL_CARGO_BIN replacement: every invocation fails so the
# harness must record build_failure and stop instead of using a stale binary.
echo "simulated cargo failure: $*" >&2
exit 1
