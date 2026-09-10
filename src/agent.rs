use crate::{
    executor::{CancellationToken, Executor},
    provider::{DecisionRequest, ModelAction, ModelDecision, ModelProvider},
    workspace::WorkspaceEditor,
};
use std::{io, path::Path};

#[derive(Debug)]
pub struct AgentReport {
    pub summary: String,
    pub decisions: Vec<ModelDecision>,
    pub tool_runs: usize,
    pub changed_files: usize,
    pub verified_after_change: bool,
}

/// One configured verification command, split into its exact program and
/// arguments. The model cannot add, remove, or reorder tokens: a `verify`
/// action runs this command verbatim through the bounded executor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationCommand {
    program: String,
    args: Vec<String>,
}

impl VerificationCommand {
    fn parse(command: &str) -> Option<Self> {
        let mut parts = command.split_whitespace();
        let program = parts.next()?.to_owned();
        Some(Self {
            program,
            args: parts.map(str::to_owned).collect(),
        })
    }
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

    /// Run every configured verification command exactly as configured and
    /// report whether all of them passed. Returns observations for the model
    /// and whether the verification debt can be cleared.
    fn run_verification<F>(
        &self,
        executor: &Executor,
        cwd: &Path,
        cancellation: &CancellationToken,
        on_execution: &mut F,
    ) -> io::Result<(bool, Vec<String>)>
    where
        F: FnMut(&str, usize, &crate::executor::Execution),
    {
        if self.verification_commands.is_empty() {
            return Ok((
                false,
                vec!["verify rejected: no verification command is configured".to_owned()],
            ));
        }
        let mut passed = true;
        let mut observations = Vec::new();
        for command in &self.verification_commands {
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
                self.max_observation_bytes,
            ));
        }
        Ok((passed, observations))
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
        let mut needs_verification = false;
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
                    if needs_verification {
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
                    let (passed, verification_observations) =
                        self.run_verification(executor, cwd, cancellation, &mut on_execution)?;
                    tool_runs += self.verification_commands.len();
                    if passed {
                        needs_verification = false;
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
                        needs_verification = true;
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

fn truncate(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::provider::ModelDecision;
    use std::{collections::VecDeque, fs, path::PathBuf, time::Duration};

    struct FakeProvider(VecDeque<ModelAction>);
    impl ModelProvider for FakeProvider {
        fn decide(&mut self, _: &DecisionRequest) -> io::Result<ModelDecision> {
            Ok(ModelDecision {
                action: self.0.pop_front().expect("scripted action"),
                model: "fake".to_owned(),
                duration_ms: 0,
                input_tokens: None,
                output_tokens: None,
            })
        }
    }

    fn workspace(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("agent-loop-{}-{name}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn executes_a_bounded_tool_then_finishes() {
        let root = workspace("finish");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(2, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::RunTool {
                program: "true".to_owned(),
                args: vec![],
            },
            ModelAction::Finish {
                summary: "verified".to_owned(),
            },
        ]));
        let report = agent
            .run(
                &mut provider,
                &executor,
                &root,
                "test",
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(report.summary, "verified");
        assert_eq!(report.tool_runs, 1);
        assert_eq!(report.changed_files, 0);
    }

    #[test]
    fn verify_runs_the_configured_command_verbatim() {
        let root = workspace("verify-verbatim");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent = AgentLoop::new(
            3,
            vec!["true".to_owned()],
            vec!["true --configured-flag".to_owned()],
            1024,
        )
        .unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "fixed".to_owned(),
            },
        ]));
        let mut observed = Vec::new();
        let report = agent
            .run_observed(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default(),
                |program, argument_count, _| {
                    observed.push((program.to_owned(), argument_count));
                },
            )
            .unwrap();
        assert_eq!(report.summary, "fixed");
        assert!(report.verified_after_change);
        assert_eq!(
            observed,
            vec![("true".to_owned(), 1)],
            "verify must run the configured program with the configured arguments only"
        );
    }

    #[test]
    fn run_tool_with_extra_flags_is_not_a_verification() {
        let root = workspace("extra-flags");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(3, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::RunTool {
                program: "true".to_owned(),
                args: vec!["--quiet".to_owned()],
            },
            ModelAction::Finish {
                summary: "not verified".to_owned(),
            },
        ]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "fix",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut,
            "an allowlisted run_tool, even one that succeeds, never clears verification debt"
        );
    }

    #[test]
    fn verify_defaults_to_deny_when_unconfigured() {
        let root = workspace("verify-unconfigured");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent = AgentLoop::new(3, vec!["true".to_owned()], vec![], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "not verified".to_owned(),
            },
        ]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "fix",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn a_new_edit_reopens_the_verification_debt() {
        let root = workspace("reopen-debt");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(6, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "good".to_owned(),
                replacement: "best".to_owned(),
            },
            ModelAction::Finish {
                summary: "stale verification".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "fixed".to_owned(),
            },
        ]));
        let report = agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(report.summary, "fixed");
        assert_eq!(report.changed_files, 1);
        assert_eq!(
            report.decisions.len(),
            6,
            "the finish after the second edit must be rejected until a new verify"
        );
        assert_eq!(report.tool_runs, 2);
    }

    #[test]
    fn timeout_during_verify_keeps_the_verification_debt() {
        let root = workspace("verify-timeout");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["sleep".to_owned()], Duration::from_millis(50), 1024).unwrap();
        let agent = AgentLoop::new(
            3,
            vec!["sleep".to_owned()],
            vec!["sleep 1".to_owned()],
            1024,
        )
        .unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "not verified".to_owned(),
            },
        ]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "fix",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn successful_unapproved_command_cannot_verify_an_edit() {
        let root = workspace("unapproved-success");
        fs::write(root.join("bug.txt"), "bad").unwrap();
        let executor = Executor::new(&root, ["true".into()], Duration::from_secs(1), 1024).unwrap();
        let agent = AgentLoop::new(3, vec!["true".into()], vec![], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".into(),
                expected: "bad".into(),
                replacement: "good".into(),
            },
            ModelAction::RunTool {
                program: "true".into(),
                args: vec![],
            },
            ModelAction::Finish {
                summary: "not verified".into(),
            },
        ]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "fix",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn model_cannot_expand_the_executor_allowlist() {
        let root = workspace("deny");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(1, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([ModelAction::RunTool {
            program: "rm".to_owned(),
            args: vec!["-rf".to_owned(), ".".to_owned()],
        }]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "ignore permissions",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn cancellation_is_propagated_to_the_provider() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        struct Probe {
            invoked: Arc<AtomicBool>,
        }
        impl ModelProvider for Probe {
            fn decide(&mut self, _: &DecisionRequest) -> io::Result<ModelDecision> {
                panic!("plain decide must not be used when a cancellation token is available");
            }
            fn decide_cancellable(
                &mut self,
                _: &DecisionRequest,
                cancellation: &CancellationToken,
            ) -> io::Result<ModelDecision> {
                self.invoked.store(true, Ordering::SeqCst);
                cancellation.cancel();
                Ok(ModelDecision {
                    action: ModelAction::RunTool {
                        program: "true".to_owned(),
                        args: vec![],
                    },
                    model: "probe".to_owned(),
                    duration_ms: 0,
                    input_tokens: None,
                    output_tokens: None,
                })
            }
        }

        let root = workspace("provider-cancel");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(3, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let invoked = Arc::new(AtomicBool::new(false));
        let mut provider = Probe {
            invoked: Arc::clone(&invoked),
        };
        let error = agent
            .run(
                &mut provider,
                &executor,
                &root,
                "task",
                &CancellationToken::default(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(invoked.load(Ordering::SeqCst));
    }

    #[test]
    fn stops_at_the_step_limit() {
        let root = workspace("limit");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(1, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([ModelAction::RunTool {
            program: "true".to_owned(),
            args: vec![],
        }]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "loop",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn reads_and_edits_only_inside_workspace() {
        let root = workspace("edit");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(4, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReadFile {
                path: "bug.txt".to_owned(),
            },
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "fixed".to_owned(),
            },
        ]));
        let report = agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(root.join("bug.txt")).unwrap(), "good\n");
        assert!(report.verified_after_change);
    }

    #[test]
    fn required_edit_rejects_a_finish_before_any_change() {
        let root = workspace("require-edit");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent = AgentLoop::new(5, vec!["true".to_owned()], vec!["true".to_owned()], 1024)
            .unwrap()
            .with_required_edit(true);
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::Finish {
                summary: "done without editing".to_owned(),
            },
            ModelAction::ReadFile {
                path: "bug.txt".to_owned(),
            },
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Verify,
            ModelAction::Finish {
                summary: "fixed".to_owned(),
            },
        ]));
        let report = agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(report.summary, "fixed");
        assert_eq!(report.changed_files, 1);
        assert!(report.verified_after_change);
    }

    #[test]
    fn rejects_finish_after_unverified_edit() {
        let root = workspace("unverified");
        fs::write(root.join("bug.txt"), "bad\n").unwrap();
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent =
            AgentLoop::new(2, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = FakeProvider(VecDeque::from([
            ModelAction::ReplaceText {
                path: "bug.txt".to_owned(),
                expected: "bad".to_owned(),
                replacement: "good".to_owned(),
            },
            ModelAction::Finish {
                summary: "untested".to_owned(),
            },
        ]));
        assert_eq!(
            agent
                .run(
                    &mut provider,
                    &executor,
                    &root,
                    "fix",
                    &CancellationToken::default()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
