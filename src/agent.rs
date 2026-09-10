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

pub struct AgentLoop {
    max_steps: usize,
    allowed_programs: Vec<String>,
    verification_programs: Vec<String>,
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
        Ok(Self {
            max_steps,
            allowed_programs,
            verification_programs,
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

    fn is_verification_command(&self, program: &str, args: &[String]) -> bool {
        self.verification_programs.iter().any(|allowed| {
            let mut parts = allowed.split_whitespace();
            parts.next() == Some(program) && parts.eq(args.iter().map(String::as_str))
        })
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
                        // Only clear the needs_verification flag if the program is a valid verification tool
                        let is_verification = self.is_verification_command(&program, &args);

                        if is_verification {
                            needs_verification = false;
                        } else {
                            observations.push("Note: This command succeeded but is not considered a formal verification of the task. Please run tests or builds.".to_string());
                        }
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
    fn verification_matches_exact_arguments_and_defaults_to_deny() {
        let agent =
            AgentLoop::new(3, vec!["cargo".into()], vec!["cargo test".into()], 1024).unwrap();
        assert!(agent.is_verification_command("cargo", &["test".into()]));
        for args in [
            vec![],
            vec!["--version".into()],
            vec!["test".into(), "--help".into()],
            vec!["test --help".into()],
        ] {
            assert!(!agent.is_verification_command("cargo", &args));
        }
        let empty = AgentLoop::new(3, vec!["true".into()], vec![], 1024).unwrap();
        assert!(!empty.is_verification_command("true", &[]));
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
            ModelAction::RunTool {
                program: "true".to_owned(),
                args: vec![],
            },
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
            ModelAction::RunTool {
                program: "true".to_owned(),
                args: vec![],
            },
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
