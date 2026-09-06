#!/bin/bash
set -euo pipefail

echo "Evaluating rust-agent-runtime..."

# Run the example
FIXTURE_DIR="$(pwd)/tests/fixtures/unwrap-panic" cargo run --example agy_fix_fixture

# Also run unit tests
cargo test --locked

echo "Evaluation complete."
