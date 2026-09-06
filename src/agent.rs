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
            let decision = provider.decide(&DecisionRequest {
                task: task.to_owned(),
                allowed_programs: self.allowed_programs.clone(),
                observations: observations.clone(),
            })?;
            let action = decision.action.clone();
            decisions.push(decision);
            match action {
                ModelAction::Finish { summary } => {
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
                    if execution.status == Some(0) && !execution.timed_out && !execution.cancelled {
                        // Only clear the needs_verification flag if the program is a valid verification tool
                        let is_verification = if self.verification_programs.is_empty() {
                            true
                        } else {
                            let mut full_cmd = program.clone();
                            if !args.is_empty() {
                                full_cmd.push(' ');
                                full_cmd.push_str(&args.join(" "));
                            }
                            self.verification_programs.contains(&full_cmd)
                        };

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
