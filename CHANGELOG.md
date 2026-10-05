# Changelog

All notable changes to this project are documented here.

## [Unreleased]

## [0.1.2] - 2026-10-05

### Added

- Prebuilt binaries for Linux (x86_64, aarch64), macOS (Intel, Apple Silicon) and Windows on each release, with SHA-256 files, plus `install.sh` and `install.ps1` installers.
- `provider::mock::MockProvider`, a deterministic scripted provider (including scripted failures)
  used by the agent and worker tests.
- Tests for provider outage handling: a provider 503 after a verified patch fails the task while
  keeping the patch and its verification trace, and the AGY adapter reports a non-zero exit or
  error envelope as a failure.

### Changed

- Split `provider.rs` into `provider/{mod,decision,agy,mock}.rs`, `agent.rs` into
  `agent/{mod,verification,observation}.rs`, and moved the event-log line format from `lib.rs`
  into `event_log.rs`. Public paths (`provider::AgyProvider`, `provider::ModelProvider`,
  `agent::AgentLoop`, and the rest) are unchanged.

### Fixed

- `AgyProvider::stream_text_cancellable` now terminates and reaps the provider process when its
  event stream is malformed, instead of leaving it running until the watchdog fires.

## [0.1.1] - 2026-09-14

### Changed

- Reworked the project overview around the current durable-runtime, provider-boundary, and
  independent-evaluation evidence.
- Removed machine-specific absolute checkout paths from tracked evaluation records.
- Gave the positive fake-provider test additional scheduling headroom while retaining the short
  deadlines in the dedicated timeout tests.

## [0.1.0] - 2026-09-12

### Added

- Durable task events, explicit retry, cancellation, and restart recovery.
- Bounded subprocess execution and structured model-provider actions.
- Post-edit verification debt and independent fixture acceptance tests.
- Deterministic evaluation controls and recorded real-model runs.
