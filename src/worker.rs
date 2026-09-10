use crate::{
    agent::{AgentLoop, AgentReport},
    executor::{CancellationToken, Executor},
    provider::ModelProvider,
    Runtime, State,
};
use std::{io, path::Path};

/// Run one durable agent task end to end.
///
/// The task is enqueued if it does not exist, marked `running` before any model
/// or tool call, and moved to a terminal state afterwards. Every completed tool
/// run is persisted as a tool trace under the same task id. Task-level traces
/// from the single-command path stay separate, so restart recovery never
/// mistakes an intermediate tool run for the terminal result of the task.
///
/// Cancellation is delivered through `cancellation`; a caller that wants
/// persistent `cancel` to reach a live worker observes the task log and signals
/// the shared token (see the `run` CLI command).
#[allow(clippy::too_many_arguments)]
pub fn run_agent_task(
    runtime: &mut Runtime,
    id: &str,
    agent: &AgentLoop,
    provider: &mut impl ModelProvider,
    executor: &Executor,
    cwd: &Path,
    task: &str,
    cancellation: &CancellationToken,
) -> io::Result<AgentReport> {
    runtime.enqueue(id)?;
    if runtime.task(id).map(|task| task.state) != Some(State::Queued) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "task is not queued (it is running or terminal)",
        ));
    }
    runtime.start(id)?;

    let mut trace_error: Option<io::Error> = None;
    let result = agent.run_observed(
        provider,
        executor,
        cwd,
        task,
        cancellation,
        |program, argument_count, execution| {
            if trace_error.is_none() {
                if let Err(error) =
                    runtime.record_tool_trace(id, program, argument_count, execution)
                {
                    trace_error = Some(error);
                }
            }
        },
    );

    if let Some(error) = trace_error {
        if runtime.task(id).map(|task| task.state) == Some(State::Running) {
            runtime.complete(id, false)?;
        }
        return Err(error);
    }

    match result {
        Ok(report) if report.changed_files > 0 && report.verified_after_change => {
            runtime.complete(id, true)?;
            Ok(report)
        }
        Ok(_) => {
            // A finish that changed no source, or changed source without a
            // passing verification, is not a demonstrated repair.
            if runtime.task(id).map(|task| task.state) == Some(State::Running) {
                runtime.complete(id, false)?;
            }
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "agent finished without a verified source change",
            ))
        }
        Err(error) => {
            let state = runtime.task(id).map(|task| task.state);
            if state == Some(State::Running) {
                if error.kind() == io::ErrorKind::Interrupted {
                    // The token was signalled; persist the cancelled state.
                    runtime.cancel(id)?;
                } else {
                    runtime.complete(id, false)?;
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        executor::Executor,
        provider::{DecisionRequest, ModelAction, ModelDecision, ModelProvider},
    };
    use std::{collections::VecDeque, fs, path::PathBuf, time::Duration};

    struct ScriptedProvider(VecDeque<ModelAction>);
    impl ModelProvider for ScriptedProvider {
        fn decide(&mut self, _: &DecisionRequest) -> io::Result<ModelDecision> {
            Ok(ModelDecision {
                action: self.0.pop_front().expect("scripted action"),
                model: "scripted".to_owned(),
                duration_ms: 0,
                input_tokens: None,
                output_tokens: None,
            })
        }
    }

    fn temp() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("worker-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn workspace_under(dir: &Path) -> PathBuf {
        let path = dir.join("workspace");
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn worker_persists_agent_flow_and_tool_traces() {
        let dir = temp();
        let workspace = workspace_under(&dir);
        fs::write(workspace.join("bug.txt"), "bad\n").unwrap();
        let executor = Executor::new(
            &workspace,
            ["true".to_owned()],
            Duration::from_secs(1),
            1024,
        )
        .unwrap();
        let agent =
            AgentLoop::new(4, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = ScriptedProvider(VecDeque::from([
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
        let mut runtime = Runtime::open(&dir).unwrap();
        let report = run_agent_task(
            &mut runtime,
            "job-1",
            &agent,
            &mut provider,
            &executor,
            &workspace,
            "fix the bug",
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(report.changed_files, 1);
        assert!(report.verified_after_change);
        assert_eq!(runtime.task("job-1").unwrap().state, State::Succeeded);
        assert_eq!(runtime.tool_traces("job-1").len(), 1);
        drop(runtime);

        let reopened = Runtime::open(&dir).unwrap();
        assert_eq!(reopened.task("job-1").unwrap().state, State::Succeeded);
        assert_eq!(reopened.tool_traces("job-1").len(), 1);
    }

    #[test]
    fn cancelled_token_marks_task_cancelled() {
        let dir = temp();
        let workspace = workspace_under(&dir);
        let executor = Executor::new(
            &workspace,
            ["true".to_owned()],
            Duration::from_secs(1),
            1024,
        )
        .unwrap();
        let agent =
            AgentLoop::new(3, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
        let mut provider = ScriptedProvider(VecDeque::new());
        let token = CancellationToken::default();
        token.cancel();
        let mut runtime = Runtime::open(&dir).unwrap();
        let error = run_agent_task(
            &mut runtime,
            "cancel-1",
            &agent,
            &mut provider,
            &executor,
            &workspace,
            "task",
            &token,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(runtime.task("cancel-1").unwrap().state, State::Cancelled);
    }

    #[test]
    fn tool_traces_do_not_finalize_a_running_task_on_restart() {
        let dir = temp();
        let workspace = workspace_under(&dir);
        let executor = Executor::new(
            &workspace,
            ["true".to_owned()],
            Duration::from_secs(1),
            1024,
        )
        .unwrap();
        {
            let mut runtime = Runtime::open(&dir).unwrap();
            runtime.enqueue("mid-1").unwrap();
            runtime.start("mid-1").unwrap();
            let execution = executor.run(&workspace, "true", &[]).unwrap();
            runtime
                .record_tool_trace("mid-1", "true", 0, &execution)
                .unwrap();
            assert_eq!(runtime.task("mid-1").unwrap().state, State::Running);
        }
        let reopened = Runtime::open(&dir).unwrap();
        assert_eq!(reopened.task("mid-1").unwrap().state, State::Queued);
        assert_eq!(reopened.tool_traces("mid-1").len(), 1);
        assert!(reopened.execution_traces("mid-1").is_empty());
    }
}
