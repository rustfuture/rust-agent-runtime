# AGY off-by-one evaluation

Date: 2026-09-06  
Model: `gemini-3.8-flash-low`  
Runtime commit under test: working tree after `9a27cd9`  
Fixture: `fixtures/off_by_one` copied to a fresh temporary directory

## Task

Fix the off-by-one defect in the small Rust crate, make the smallest correct source change, and verify it with `cargo test`.

## Baseline

The unchanged fixture failed its only test. For `[10]`, `last_index` returned `Some(1)` while the expected last valid index was `Some(0)`.

## Result

- Outcome: success on 1/1 task.
- Patch: `Some(values.len())` became `Some(values.len() - 1)`; no other source line changed.
- Independent post-run test: 1 unit test and 0 doc tests passed.
- Model decisions: 6.
- Bounded tool runs: 3.
- Provider duration: 55,565 ms total.
- AGY-reported tokens: 214,886 input; 3,051 output.
- Reported monetary cost: unavailable from AGY, so no cost claim is made.

This is one deliberately small fixture and does not establish a general success rate. Failed tasks must be included before a broader success-rate claim is made.

## Bounded-budget failure case

The same unchanged fixture was run in a fresh copy with `MAX_STEPS=1`. The run exited non-zero with `TimedOut: agent step limit reached` before it could produce a verified patch. This is recorded as a failed task, not hidden as a tool error. The resulting current evaluation set is therefore 1 completed / 2 total (50%); it is a deliberately tiny smoke set and not a portfolio-wide success claim.

## Reproduction

Copy only `Cargo.toml`, `Cargo.lock`, and `src/lib.rs` from the fixture into a fresh directory, then run:

```bash
provider_dir=$(mktemp -d)
AGY_BIN=/path/to/agy \
AGY_WORK_DIR="$provider_dir" \
FIXTURE_DIR=/path/to/fresh-fixture-copy \
cargo run --locked --example agy_fix_fixture
cargo test --manifest-path /path/to/fresh-fixture-copy/Cargo.toml --locked
```

AGY is deliberately run in a separate empty directory. The model receives repository content only through the runtime's bounded read action, and requested commands still pass through the Rust executor.
