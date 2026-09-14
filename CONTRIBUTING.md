# Contributing

Keep changes narrow and preserve the separation between model requests and runtime authority.

Before opening a pull request, run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash evaluation/run_controls.sh
```

Live provider calls are not required for ordinary changes. If a change affects provider parsing or
the evaluation harness, include deterministic coverage and describe any separately executed live
evaluation without hiding failures.

Do not commit API credentials, raw private repository content, or machine-specific absolute paths.
