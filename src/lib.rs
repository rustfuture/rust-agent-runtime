pub mod executor;

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

pub struct Runtime {
    tasks: HashMap<String, Task>,
    log: PathBuf,
}

impl Runtime {
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let log = data_dir.join("events.log");
        let mut runtime = Self {
            tasks: HashMap::new(),
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
            runtime.transition(&id, State::Queued, "restart_recovery")?;
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

    fn apply_line(&mut self, line: &str) {
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
    fn temp() -> PathBuf {
        std::env::temp_dir()
            .join(format!("agent-runtime-{}", std::process::id()))
            .join(std::thread::current().name().unwrap_or("test"))
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
}
