mod observation;
#[cfg(all(test, unix))]
mod tests;
mod verification;

use crate::{
    executor::{CancellationToken, Executor},
    provider::{DecisionRequest, ModelAction, ModelDecision, ModelProvider},
    workspace::WorkspaceEditor,
};
use observation::truncate;
use std::{io, path::Path};
use verification::{run_verification, VerificationCommand, VerificationDebt};

#[derive(Debug)]
pub struct AgentReport {
    pub summary: String,
    pub decisions: Vec<ModelDecision>,
    pub tool_runs: usize,
    pub changed_files: usize,
    pub verified_after_change: bool,
}

pub struct AgentLoop {
    max_steps: usize,
    allowed_programs: Vec<String>,
    verification_programs: Vec<String>,
    verification_commands: Vec<VerificationCommand>,
    max_observation_bytes: usize,
    require_edit: bool,
}

impl AgentLoop {
    pub fn new(
        max_steps: usize,
        allowed_programs: Vec<String>,
        verification_programs: Vec<String>,
        max_observation_bytes: usize,
    ) -> io::Result<Self> {
        if max_steps == 0 || max_observation_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "limits must be positive",
            ));
        }
        let verification_commands = verification_programs
            .iter()
            .filter_map(|command| VerificationCommand::parse(command))
            .collect();
        Ok(Self {
            max_steps,
            allowed_programs,
            verification_programs,
            verification_commands,
            max_observation_bytes,
            require_edit: false,
        })
    }

    /// When enabled, a `finish` is rejected until at least one edit has been
    /// applied. The default is disabled so single-command tasks keep working.
    pub fn with_required_edit(mut self, require_edit: bool) -> Self {
        self.require_edit = require_edit;
        self
    }

    pub fn run(
        &self,
        provider: &mut impl ModelProvider,
        executor: &Executor,
        cwd: &Path,
        task: &str,
        cancellation: &CancellationToken,
    ) -> io::Result<AgentReport> {
        self.run_observed(provider, executor, cwd, task, cancellation, |_, _, _| {})
    }

    /// Same as [`AgentLoop::run`], but reports every completed tool run to
    /// `on_execution` so a durable worker can persist per-command traces.
    pub fn run_observed<F>(
        &self,
        provider: &mut impl ModelProvider,
        executor: &Executor,
        cwd: &Path,
        task: &str,
        cancellation: &CancellationToken,
        mut on_execution: F,
    ) -> io::Result<AgentReport>
    where
        F: FnMut(&str, usize, &crate::executor::Execution),
    {
        let mut observations = Vec::new();
        let editor = WorkspaceEditor::new(cwd, self.max_observation_bytes)?;
        let mut decisions = Vec::new();
        let mut tool_runs = 0;
        let mut changed_files = std::collections::HashSet::new();
        let mut debt = VerificationDebt::default();
        for _ in 0..self.max_steps {
            if cancellation.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "agent cancelled",
                ));
            }
            let decision = provider.decide_cancellable(
                &DecisionRequest {
                    task: task.to_owned(),
                    allowed_programs: self.allowed_programs.clone(),
                    verification_programs: self.verification_programs.clone(),
                    observations: observations.clone(),
                },
                cancellation,
            )?;
            let action = decision.action.clone();
            decisions.push(decision);
            match action {
                ModelAction::Finish { summary } => {
                    if self.require_edit && changed_files.is_empty() {
                        observations.push(
                            "finish rejected: no source edit has been applied yet; use read_file then replace_text to make the change"
                                .to_owned(),
                        );
                        continue;
                    }
                    if debt.is_open() {
                        observations.push(
                            "finish rejected: run a successful verification command after the latest edit"
                                .to_owned(),
                        );
                        continue;
                    }
                    return Ok(AgentReport {
                        summary,
                        decisions,
                        tool_runs,
                        changed_files: changed_files.len(),
                        verified_after_change: !changed_files.is_empty(),
                    });
                }
                ModelAction::RunTool { program, args } => {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let execution = executor.run_cancellable(cwd, &program, &refs, cancellation)?;
                    tool_runs += 1;
                    on_execution(&program, args.len(), &execution);
                    if execution.status == Some(0) && !execution.timed_out && !execution.cancelled {
                        observations.push(
                            "Note: run_tool never clears verification debt; use the verify action to run the configured verification command."
                                .to_owned(),
                        );
                    }
                    let observation = format!(
                        "program={program} status={:?} timeout={} cancelled={} stdout={} stderr={}",
                        execution.status,
                        execution.timed_out,
                        execution.cancelled,
                        execution.stdout,
                        execution.stderr
                    );
                    observations.push(truncate(observation, self.max_observation_bytes));
                }
                ModelAction::Verify => {
                    let (passed, verification_observations) = run_verification(
                        &self.verification_commands,
                        self.max_observation_bytes,
                        executor,
                        cwd,
                        cancellation,
                        &mut on_execution,
                    )?;
                    tool_runs += self.verification_commands.len();
                    if passed {
                        debt.clear();
                    }
                    observations.extend(verification_observations);
                }
                ModelAction::ReadFile { path } => match editor.read(&path) {
                    Ok(content) => {
                        observations.push(truncate(
                            format!("file={path}\n{content}"),
                            self.max_observation_bytes,
                        ));
                    }
                    Err(error) => {
                        observations.push(format!("read_file failed for file={path}: {error}"));
                    }
                },
                ModelAction::ReplaceText {
                    path,
                    expected,
                    replacement,
                } => match editor.replace_once(&path, &expected, &replacement) {
                    Ok(()) => {
                        changed_files.insert(path.clone());
                        debt.open();
                        observations.push(format!("replaced exact text in file={path}"));
                    }
                    Err(error) => {
                        observations.push(format!("replace_text failed in file={path}: {error}"));
                    }
                },
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "agent step limit reached",
        ))
    }
}
