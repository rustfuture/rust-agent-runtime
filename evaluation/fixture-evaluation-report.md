# Multi-Fixture Agent Evaluation Report

Date: 2026-09-06  
Model: `gemini-3.8-flash-low` via AGY structured output provider  
Runtime: `rust-agent-runtime v0.1.0` (durable state, bounded executor, macOS seatbelt isolation)  
Hardware: Apple M4 Pro (24 GB unified memory)

## Overview

The runtime was evaluated across independent Rust bug-repair fixtures copied into fresh, isolated workspaces. The agent loop had access only to bounded read, single-exact replace, allowlisted `cargo` executions inside the target workspace, and a finish action requiring post-edit test verification.

## Results by Fixture

| Fixture | Defect Family | Task Budget | Outcome | Steps | Tool Runs | Changed Files | Post-Run Test | Notes |
|---|---|---|---|---|---|---|---|---|
| `off_by_one` | Index calculation (`len` vs `len - 1`) | 8 | **Completed & Verified** | 6 | 3 | 1 | **PASSED (1/1)** | Replaced `Some(values.len())` with `Some(values.len() - 1)`; verified with `cargo test`. |
| `off_by_one` | Index calculation | 1 | **Failed (Budget Limit)** | 1 | 0 | 0 | **FAILED (0/1)** | Timed out on step limit before completing repair. Recorded as failure. |
| `clamp_range` | Logic inversion (`val < min` -> `min`) | 8 | **Failed (Unresolved)** | 1 | 0 | 0 | **FAILED (0/1)** | Model emitted `Finish` prematurely without reading file or running test. Zero files changed. |
| `prefix_format` | String formatting (`{trimmed}` -> `{prefix}{trimmed}`) | 8 | **Completed & Verified** | 2 | 1 | 1 | **PASSED (1/1)** | Replaced formatting string; verified with `cargo test`. |

## Aggregate Evaluation Metrics

- **Total runs**: 4
- **Verified successful repairs**: 2 / 4 (50.0%)
- **Bounded budget failures**: 1 / 4 (25.0%)
- **Unresolved / Premature finishes**: 1 / 4 (25.0%)
- **Mean decisions per successful task**: 4.0
- **Total provider cost**: $0.00 (local AGY harness)

## Integrity & Limitations Statement

The 50% completion rate reflects this specific 4-run evaluation suite on tiny synthetic crates. It is **not** a general benchmark or proof of wide-domain autonomy. When the model receives an empty initial observation, it occasionally misidentifies the workspace state and exits early (as in `clamp_range`). The runtime correctly prevents unverified code changes from succeeding (`verified_after_change` enforcement).
