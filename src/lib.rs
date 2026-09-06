pub mod agent;
pub mod executor;
pub mod provider;

use executor::{CancellationToken, Execution, Executor};

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub state: State,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTrace {
    pub attempt: u32,
    pub program: String,
    pub argument_count: usize,
    pub status: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub output_truncated: bool,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub duration_ms: u128,
}

pub struct Runtime {
    tasks: HashMap<String, Task>,
    traces: HashMap<String, Vec<ExecutionTrace>>,
    log: PathBuf,
}

impl Runtime {
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let log = data_dir.join("events.log");
        let mut runtime = Self {
            tasks: HashMap::new(),
            traces: HashMap::new(),
            log,
        };
        if runtime.log.exists() {
            for line in fs::read_to_string(&runtime.log)?.lines() {
                runtime.apply_line(line);
            }
        }
        let interrupted: Vec<String> = runtime
            .tasks
            .values()
            .filter(|task| task.state == State::Running)
            .map(|task| task.id.clone())
            .collect();
        for id in interrupted {
            let task_attempt = runtime.tasks[&id].attempts;
            let completed = runtime
                .traces
                .get(&id)
                .and_then(|traces| traces.last())
                .filter(|trace| trace.attempt == task_attempt)
                .map(trace_state);
            if let Some(state) = completed {
                runtime.transition(&id, state, "restart_trace_recovery")?;
            } else {
                runtime.transition(&id, State::Queued, "restart_recovery")?;
            }
        }
        Ok(runtime)
    }

    pub fn enqueue(&mut self, id: &str) -> io::Result<bool> {
        if self.tasks.contains_key(id) {
            return Ok(false);
        }
        self.record(id, State::Queued, 0, "enqueue")?;
        Ok(true)
    }

    pub fn start(&mut self, id: &str) -> io::Result<()> {
        let task = self
            .tasks
            .get(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;
        if task.state != State::Queued {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "task is not queued",
            ));
        }
        self.record(id, State::Running, task.attempts + 1, "start")
    }

    pub fn complete(&mut self, id: &str, success: bool) -> io::Result<()> {
        self.transition(
            id,
            if success {
                State::Succeeded
            } else {
                State::Failed
            },
            "complete",
        )
    }
    pub fn cancel(&mut self, id: &str) -> io::Result<()> {
        self.transition(id, State::Cancelled, "cancel")
    }
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    pub fn execution_traces(&self, id: &str) -> &[ExecutionTrace] {
        self.traces.get(id).map(Vec::as_slice).unwrap_or_default()
    }

    /// Requeues a failed task if its completed attempt count is below `max_attempts`.
    /// The caller remains responsible for ensuring the command is safe to repeat.
    pub fn retry(&mut self, id: &str, max_attempts: u32) -> io::Result<bool> {
        let task = self
            .tasks
            .get(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;
        if task.state != State::Failed || task.attempts >= max_attempts {
            return Ok(false);
        }
        self.transition(id, State::Queued, "retry")?;
        Ok(true)
    }

    pub fn next_queued(&self) -> Option<&Task> {
        self.tasks
            .values()
            .filter(|task| task.state == State::Queued)
            .min_by(|a, b| a.id.cmp(&b.id))
    }

    pub fn run_next(
        &mut self,
        executor: &Executor,
        cwd: &Path,
        program: &str,
        args: &[&str],
    ) -> io::Result<Option<(String, Execution)>> {
        self.run_next_cancellable(executor, cwd, program, args, &CancellationToken::default())
    }

    pub fn run_next_cancellable(
        &mut self,
        executor: &Executor,
        cwd: &Path,
        program: &str,
        args: &[&str],
        cancellation: &CancellationToken,
    ) -> io::Result<Option<(String, Execution)>> {
        let Some(id) = self.next_queued().map(|task| task.id.clone()) else {
            return Ok(None);
        };
        self.start(&id)?;
        match executor.run_cancellable(cwd, program, args, cancellation) {
            Ok(execution) => {
                self.record_execution(&id, program, args.len(), &execution)?;
                if execution.cancelled {
                    self.cancel(&id)?;
                    return Ok(Some((id, execution)));
                }
                let success = execution.status == Some(0) && !execution.timed_out;
                self.complete(&id, success)?;
                Ok(Some((id, execution)))
            }
            Err(error) => {
                self.complete(&id, false)?;
                Err(error)
            }
        }
    }

    fn transition(&mut self, id: &str, state: State, reason: &str) -> io::Result<()> {
        let attempts = self
            .tasks
            .get(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?
            .attempts;
        self.record(id, state, attempts, reason)
    }

    fn record(&mut self, id: &str, state: State, attempts: u32, reason: &str) -> io::Result<()> {
        if id.contains(['\t', '\n']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid task id",
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        writeln!(file, "{id}\t{}\t{attempts}\t{reason}", state.as_str())?;
        file.sync_data()?;
        self.tasks.insert(
            id.to_owned(),
            Task {
                id: id.to_owned(),
                state,
                attempts,
            },
        );
        Ok(())
    }

    fn record_execution(
        &mut self,
        id: &str,
        program: &str,
        argument_count: usize,
        execution: &Execution,
    ) -> io::Result<()> {
        if id.contains(['\t', '\n']) || program.contains(['\t', '\n']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid trace field",
            ));
        }
        let attempt = self
            .tasks
            .get(id)
            .map(|task| task.attempts)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;
        let trace = ExecutionTrace {
            attempt,
            program: program.to_owned(),
            argument_count,
            status: execution.status,
            timed_out: execution.timed_out,
            cancelled: execution.cancelled,
            output_truncated: execution.output_truncated,
            stdout_bytes: execution.stdout.len(),
            stderr_bytes: execution.stderr.len(),
            duration_ms: execution.duration_ms,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        writeln!(
            file,
            "trace\t{id}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            trace.attempt,
            trace.program,
            trace.argument_count,
            trace
                .status
                .map_or_else(|| "none".to_owned(), |value| value.to_string()),
            trace.timed_out,
            trace.cancelled,
            trace.output_truncated,
            trace.stdout_bytes,
            trace.stderr_bytes,
            trace.duration_ms
        )?;
        file.sync_data()?;
        self.traces.entry(id.to_owned()).or_default().push(trace);
        Ok(())
    }

    fn apply_line(&mut self, line: &str) {
        if let Some(trace) = parse_trace(line) {
            self.traces.entry(trace.0).or_default().push(trace.1);
            return;
        }
        let mut parts = line.split('\t');
        let (Some(id), Some(state), Some(attempts)) = (parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        let (Some(state), Ok(attempts)) = (State::parse(state), attempts.parse()) else {
            return;
        };
        self.tasks.insert(
            id.to_owned(),
            Task {
                id: id.to_owned(),
                state,
                attempts,
            },
        );
    }
}

fn parse_trace(line: &str) -> Option<(String, ExecutionTrace)> {
    let mut parts = line.split('\t');
    if parts.next()? != "trace" {
        return None;
    }
    let id = parts.next()?.to_owned();
    let attempt = parts.next()?.parse().ok()?;
    let program = parts.next()?.to_owned();
    let argument_count = parts.next()?.parse().ok()?;
    let status_text = parts.next()?;
    let status = if status_text == "none" {
        None
    } else {
        Some(status_text.parse().ok()?)
    };
    let timed_out = parts.next()?.parse().ok()?;
    let cancelled = parts.next()?.parse().ok()?;
    let output_truncated = parts.next()?.parse().ok()?;
    let stdout_bytes = parts.next()?.parse().ok()?;
    let stderr_bytes = parts.next()?.parse().ok()?;
    let duration_ms = parts.next()?.parse().ok()?;
    Some((
        id,
        ExecutionTrace {
            attempt,
            program,
            argument_count,
            status,
            timed_out,
            cancelled,
            output_truncated,
            stdout_bytes,
            stderr_bytes,
            duration_ms,
        },
    ))
}

fn trace_state(trace: &ExecutionTrace) -> State {
    if trace.cancelled {
        State::Cancelled
    } else if trace.status == Some(0) && !trace.timed_out {
        State::Succeeded
    } else {
        State::Failed
    }
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-runtime-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn duplicate_enqueue_is_idempotent() {
        let dir = temp();
        let mut rt = Runtime::open(&dir).unwrap();
        assert!(rt.enqueue("a").unwrap());
        assert!(!rt.enqueue("a").unwrap());
    }

    #[test]
    fn interrupted_running_task_is_requeued() {
        let dir = temp();
        {
            let mut rt = Runtime::open(&dir).unwrap();
            rt.enqueue("b").unwrap();
            rt.start("b").unwrap();
        }
        let rt = Runtime::open(&dir).unwrap();
        assert_eq!(rt.task("b").unwrap().state, State::Queued);
        assert_eq!(rt.task("b").unwrap().attempts, 1);
    }

    #[test]
    fn cancellation_is_durable() {
        let dir = temp();
        {
            let mut rt = Runtime::open(&dir).unwrap();
            rt.enqueue("c").unwrap();
            rt.cancel("c").unwrap();
        }
        let rt = Runtime::open(&dir).unwrap();
        assert_eq!(rt.task("c").unwrap().state, State::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn worker_persists_bounded_execution_result() {
        let dir = temp();
        let workspace = dir.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let executor = Executor::new(
            &workspace,
            ["true".to_owned()],
            std::time::Duration::from_secs(1),
            1024,
        )
        .unwrap();
        {
            let mut rt = Runtime::open(&dir).unwrap();
            rt.enqueue("work-1").unwrap();
            let (id, execution) = rt
                .run_next(&executor, &workspace, "true", &[])
                .unwrap()
                .unwrap();
            assert_eq!(id, "work-1");
            assert_eq!(execution.status, Some(0));
            assert_eq!(rt.task("work-1").unwrap().state, State::Succeeded);
        }
        let rt = Runtime::open(&dir).unwrap();
        assert_eq!(rt.task("work-1").unwrap().state, State::Succeeded);
        assert!(rt.next_queued().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn worker_persists_cancellation() {
        let dir = temp();
        let workspace = dir.join("cancel-workspace");
        fs::create_dir_all(&workspace).unwrap();
        let executor = Executor::new(
            &workspace,
            ["sleep".to_owned()],
            std::time::Duration::from_secs(2),
            1024,
        )
        .unwrap();
        let token = CancellationToken::default();
        token.cancel();
        let mut rt = Runtime::open(&dir).unwrap();
        rt.enqueue("cancel-work").unwrap();
        let (_, execution) = rt
            .run_next_cancellable(&executor, &workspace, "sleep", &["1"], &token)
            .unwrap()
            .unwrap();
        assert!(execution.cancelled);
        assert_eq!(rt.task("cancel-work").unwrap().state, State::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn execution_trace_survives_restart_without_argument_contents() {
        let dir = temp();
        let workspace = dir.join("trace-workspace");
        fs::create_dir_all(&workspace).unwrap();
        let executor = Executor::new(
            &workspace,
            ["printf".to_owned()],
            std::time::Duration::from_secs(1),
            1024,
        )
        .unwrap();
        {
            let mut rt = Runtime::open(&dir).unwrap();
            rt.enqueue("trace-work").unwrap();
            rt.run_next(&executor, &workspace, "printf", &["sensitive-value"])
                .unwrap();
            let trace = &rt.execution_traces("trace-work")[0];
            assert_eq!(trace.attempt, 1);
            assert_eq!(trace.program, "printf");
            assert_eq!(trace.argument_count, 1);
            assert_eq!(trace.stdout_bytes, 15);
        }
        let rt = Runtime::open(&dir).unwrap();
        assert_eq!(rt.execution_traces("trace-work").len(), 1);
        assert!(!fs::read_to_string(dir.join("events.log"))
            .unwrap()
            .contains("sensitive-value"));
    }

    #[cfg(unix)]
    #[test]
    fn retry_is_bounded_and_records_distinct_attempts() {
        let dir = temp();
        let workspace = dir.join("retry-workspace");
        fs::create_dir_all(&workspace).unwrap();
        let executor = Executor::new(
            &workspace,
            ["false".to_owned()],
            std::time::Duration::from_secs(1),
            1024,
        )
        .unwrap();
        let mut rt = Runtime::open(&dir).unwrap();
        rt.enqueue("retry-work").unwrap();
        rt.run_next(&executor, &workspace, "false", &[]).unwrap();
        assert!(rt.retry("retry-work", 2).unwrap());
        rt.run_next(&executor, &workspace, "false", &[]).unwrap();
        assert!(!rt.retry("retry-work", 2).unwrap());
        assert_eq!(rt.task("retry-work").unwrap().attempts, 2);
        assert_eq!(
            rt.execution_traces("retry-work")
                .iter()
                .map(|trace| trace.attempt)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[cfg(unix)]
    #[test]
    fn restart_uses_completed_trace_instead_of_repeating_work() {
        let dir = temp();
        let workspace = dir.join("recovery-workspace");
        fs::create_dir_all(&workspace).unwrap();
        let executor = Executor::new(
            &workspace,
            ["true".to_owned()],
            std::time::Duration::from_secs(1),
            1024,
        )
        .unwrap();
        {
            let mut rt = Runtime::open(&dir).unwrap();
            rt.enqueue("recovery-work").unwrap();
            rt.start("recovery-work").unwrap();
            let execution = executor.run(&workspace, "true", &[]).unwrap();
            rt.record_execution("recovery-work", "true", 0, &execution)
                .unwrap();
            assert_eq!(rt.task("recovery-work").unwrap().state, State::Running);
        }
        let rt = Runtime::open(&dir).unwrap();
        assert_eq!(rt.task("recovery-work").unwrap().state, State::Succeeded);
        assert!(rt.next_queued().is_none());
        assert_eq!(rt.execution_traces("recovery-work").len(), 1);
    }
}
