# Reference

This document provides extended reference details relocated from the main README.

## Repository Map

| Path | Contents |
|---|---|
| `src/lib.rs`, `src/worker.rs` | Event-sourced task state, recovery, worker loop |
| `src/agent.rs` | Step-bounded agent loop and verification debt |
| `src/executor.rs` | Bounded subprocess execution and process groups |
| `src/provider.rs` | AGY adapter, timeouts, output caps, stream parsing |
| `src/workspace.rs` | Workspace containment and exact-match edits |
| `src/main.rs` | Terminal interface (`enqueue`, `run`, `cancel`, `status`, `watch`) |
| `evaluation/` | Fixtures, reports, and deterministic controls |
| `examples/` | AGY adapter smoke examples |

## Provider Smoke Testing

Real provider calls are opt-in and excluded from normal CI:

```bash
provider_dir=$(mktemp -d)
AGY_BIN=/path/to/agy AGY_WORK_DIR="$provider_dir" \
  cargo run --locked --example agy_smoke
```

`AgyProvider::stream_text` consumes AGY's newline-delimited event stream while retaining final token and latency metadata; see [`examples/agy_stream_smoke.rs`](../examples/agy_stream_smoke.rs).

## Historical Evaluation Evidence

The earlier [`evaluation/fixture-evaluation-report.md`](../evaluation/fixture-evaluation-report.md) is retained as historical pre-hardening evidence.
