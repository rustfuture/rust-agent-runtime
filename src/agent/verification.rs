//! Verification debt: the configured verification command and the bounded
//! run of it that a `verify` action triggers.

use super::observation::truncate;
use crate::executor::{CancellationToken, Execution, Executor};
use std::{io, path::Path};

/// One configured verification command, split into its exact program and
/// arguments. The model cannot add, remove, or reorder tokens: a `verify`
/// action runs this command verbatim through the bounded executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerificationCommand {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl VerificationCommand {
    pub(super) fn parse(command: &str) -> Option<Self> {
        let mut parts = command.split_whitespace();
        let program = parts.next()?.to_owned();
        Some(Self {
            program,
            args: parts.map(str::to_owned).collect(),
        })
    }
}

/// Tracks whether a workspace edit has not yet been followed by a passing
/// verification. Only a successful `verify` clears it; a new edit reopens it.
#[derive(Debug, Default)]
pub(super) struct VerificationDebt {
    open: bool,
}

impl VerificationDebt {
    pub(super) fn open(&mut self) {
        self.open = true;
    }

    pub(super) fn clear(&mut self) {
        self.open = false;
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }
}

/// Run every configured verification command exactly as configured and
/// report whether all of them passed, with one observation per command.
pub(super) fn run_verification<F>(
    commands: &[VerificationCommand],
    max_observation_bytes: usize,
    executor: &Executor,
    cwd: &Path,
    cancellation: &CancellationToken,
    on_execution: &mut F,
) -> io::Result<(bool, Vec<String>)>
where
    F: FnMut(&str, usize, &Execution),
{
    if commands.is_empty() {
        return Ok((
            false,
            vec!["verify rejected: no verification command is configured".to_owned()],
        ));
    }
    let mut passed = true;
    let mut observations = Vec::new();
    for command in commands {
        let refs: Vec<&str> = command.args.iter().map(String::as_str).collect();
        let execution = executor.run_cancellable(cwd, &command.program, &refs, cancellation)?;
        on_execution(&command.program, command.args.len(), &execution);
        let command_passed =
            execution.status == Some(0) && !execution.timed_out && !execution.cancelled;
        passed &= command_passed;
        observations.push(truncate(
            format!(
                "verify program={} status={:?} timeout={} cancelled={} stdout={} stderr={}",
                command.program,
                execution.status,
                execution.timed_out,
                execution.cancelled,
                execution.stdout,
                execution.stderr
            ),
            max_observation_bytes,
        ));
    }
    Ok((passed, observations))
}
