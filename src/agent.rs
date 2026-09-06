use crate::{
    executor::{CancellationToken, Executor},
    provider::{DecisionRequest, ModelAction, ModelDecision, ModelProvider},
};
use std::{io, path::Path};

#[derive(Debug)]
pub struct AgentReport {
    pub summary: String,
    pub decisions: Vec<ModelDecision>,
    pub tool_runs: usize,
}

pub struct AgentLoop {
    max_steps: usize,
    allowed_programs: Vec<String>,
    max_observation_bytes: usize,
}

impl AgentLoop {
    pub fn new(
        max_steps: usize,
        allowed_programs: Vec<String>,
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
        let mut decisions = Vec::new();
        let mut tool_runs = 0;
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
                    return Ok(AgentReport {
                        summary,
                        decisions,
                        tool_runs,
                    });
                }
                ModelAction::RunTool { program, args } => {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let execution = executor.run_cancellable(cwd, &program, &refs, cancellation)?;
                    tool_runs += 1;
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
        let agent = AgentLoop::new(2, vec!["true".to_owned()], 1024).unwrap();
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
    }

    #[test]
    fn model_cannot_expand_the_executor_allowlist() {
        let root = workspace("deny");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        let agent = AgentLoop::new(1, vec!["true".to_owned()], 1024).unwrap();
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
        let agent = AgentLoop::new(1, vec!["true".to_owned()], 1024).unwrap();
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
}
