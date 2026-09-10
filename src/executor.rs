use std::{
    collections::HashSet,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[derive(Debug)]
pub struct Execution {
    pub status: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub duration_ms: u128,
}

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct Executor {
    workspace: PathBuf,
    allowed: HashSet<String>,
    timeout: Duration,
    max_output_bytes: usize,
    isolation: Isolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Isolation {
    #[default]
    None,
    MacOsSandbox,
}

impl Executor {
    pub fn new(
        workspace: &Path,
        allowed: impl IntoIterator<Item = String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> io::Result<Self> {
        Ok(Self {
            workspace: workspace.canonicalize()?,
            allowed: allowed.into_iter().collect(),
            timeout,
            max_output_bytes,
            isolation: Isolation::None,
        })
    }

    pub fn with_isolation(mut self, isolation: Isolation) -> io::Result<Self> {
        if isolation == Isolation::MacOsSandbox && !Path::new("/usr/bin/sandbox-exec").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "macOS sandbox-exec is unavailable",
            ));
        }
        self.isolation = isolation;
        Ok(self)
    }

    pub fn run(&self, cwd: &Path, program: &str, args: &[&str]) -> io::Result<Execution> {
        self.run_cancellable(cwd, program, args, &CancellationToken::default())
    }

    pub fn run_cancellable(
        &self,
        cwd: &Path,
        program: &str,
        args: &[&str],
        cancellation: &CancellationToken,
    ) -> io::Result<Execution> {
        if program.contains('/') || !self.allowed.contains(program) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "program is not allowlisted",
            ));
        }
        let cwd = cwd.canonicalize()?;
        if cwd != self.workspace && !cwd.starts_with(&self.workspace) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "working directory escapes workspace",
            ));
        }

        let started = Instant::now();
        let mut command = match self.isolation {
            Isolation::None => {
                let mut command = Command::new(program);
                command.args(args);
                command
            }
            Isolation::MacOsSandbox => {
                let profile = macos_profile(&self.workspace);
                let mut command = Command::new("/usr/bin/sandbox-exec");
                command.args(["-p", &profile, program]).args(args);
                command
            }
        };
        isolate_process_group(&mut command);
        let mut child = command
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let max_bytes = self.max_output_bytes;
        let (stdout_data, stdout_done) = spawn_pipe_reader(child.stdout.take().unwrap(), max_bytes);
        let (stderr_data, stderr_done) = spawn_pipe_reader(child.stderr.take().unwrap(), max_bytes);
        let mut timed_out = false;
        let mut cancelled = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status.code();
            }
            if cancellation.is_cancelled() {
                cancelled = true;
                terminate_child(&mut child)?;
                break child.wait()?.code();
            }
            if started.elapsed() >= self.timeout {
                timed_out = true;
                terminate_child(&mut child)?;
                break child.wait()?.code();
            }
            thread::sleep(Duration::from_millis(10));
        };
        // A descendant that inherited the pipe keeps the write end open after the
        // direct child exits, so the reader may never see EOF. Wait only a bounded
        // grace period, then return the captured output instead of blocking.
        let grace = Duration::from_millis(500);
        let _ = stdout_done.recv_timeout(grace);
        let _ = stderr_done.recv_timeout(grace);
        let (stdout, stdout_truncated) = {
            let data = stdout_data.lock().unwrap();
            (
                String::from_utf8_lossy(&data.bytes).into_owned(),
                data.truncated,
            )
        };
        let (stderr, stderr_truncated) = {
            let data = stderr_data.lock().unwrap();
            (
                String::from_utf8_lossy(&data.bytes).into_owned(),
                data.truncated,
            )
        };
        Ok(Execution {
            status,
            timed_out,
            cancelled,
            stdout,
            stderr,
            output_truncated: stdout_truncated || stderr_truncated,
            duration_ms: started.elapsed().as_millis(),
        })
    }
}

#[derive(Default)]
pub(crate) struct PipeData {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

/// Reads a child pipe into a shared buffer up to `max_bytes`, then signals
/// completion. Leaving the returned receiver un-drained never blocks the caller.
pub(crate) fn spawn_pipe_reader<R: Read + Send + 'static>(
    mut reader: R,
    max_bytes: usize,
) -> (
    Arc<std::sync::Mutex<PipeData>>,
    std::sync::mpsc::Receiver<()>,
) {
    let shared = Arc::new(std::sync::Mutex::new(PipeData::default()));
    let writer = Arc::clone(&shared);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        let mut total = 0usize;
        loop {
            let remaining = max_bytes.saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let to_read = remaining.min(chunk.len());
            match reader.read(&mut chunk[..to_read]) {
                Ok(0) => break,
                Ok(n) => {
                    writer.lock().unwrap().bytes.extend_from_slice(&chunk[..n]);
                    total += n;
                }
                Err(_) => break,
            }
        }
        if total >= max_bytes {
            let mut tiny = [0u8; 1];
            if let Ok(1) = reader.read(&mut tiny) {
                writer.lock().unwrap().truncated = true;
            }
        }
        let _ = done_tx.send(());
    });
    (shared, done_rx)
}

/// Put the child in its own process group so a timeout or cancellation can
/// terminate it and its descendants without signalling the worker.
pub(crate) fn isolate_process_group(_command: &mut std::process::Command) {
    #[cfg(unix)]
    unsafe {
        _command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub(crate) fn terminate_child(child: &mut std::process::Child) -> io::Result<()> {
    #[cfg(unix)]
    {
        // The child creates its own process group in pre_exec; a negative pid
        // terminates the direct child and descendants without touching the worker.
        let result = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    child.kill()
}

fn macos_profile(workspace: &Path) -> String {
    let escaped = workspace
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n(allow file-write* (subpath \"{escaped}\"))"
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    fn workspace(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("executor-{}-{name}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn denies_non_allowlisted_program_and_outside_cwd() {
        let root = workspace("deny");
        let executor =
            Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
        assert_eq!(
            executor.run(&root, "false", &[]).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            executor
                .run(Path::new("/"), "true", &[])
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn enforces_timeout_and_output_limit() {
        let root = workspace("limits");
        let executor = Executor::new(
            &root,
            ["sleep".to_owned(), "printf".to_owned()],
            Duration::from_millis(50),
            5,
        )
        .unwrap();
        let timeout = executor.run(&root, "sleep", &["1"]).unwrap();
        assert!(timeout.timed_out);
        let output = executor.run(&root, "printf", &["1234567890"]).unwrap();
        assert_eq!(output.stdout, "12345");
        assert!(output.output_truncated);
    }

    #[test]
    fn cancellation_kills_running_process() {
        let root = workspace("cancel");
        let executor =
            Executor::new(&root, ["sleep".to_owned()], Duration::from_secs(5), 1024).unwrap();
        let token = CancellationToken::default();
        let trigger = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            trigger.cancel();
        });
        let result = executor
            .run_cancellable(&root, "sleep", &["2"], &token)
            .unwrap();
        thread.join().unwrap();
        assert!(result.cancelled);
        assert!(!result.timed_out);
        assert!(result.duration_ms < 1000);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_descendant_process_group() {
        let root = workspace("process-group");
        let marker = root.join("descendant-marker");
        let executor =
            Executor::new(&root, ["sh".to_owned()], Duration::from_millis(50), 1024).unwrap();
        let script = format!("sleep 1; touch {}", marker.display());
        let result = executor.run(&root, "sh", &["-c", &script]).unwrap();
        assert!(result.timed_out);
        std::thread::sleep(Duration::from_millis(150));
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn returns_without_waiting_for_a_descendant_holding_the_pipe() {
        let root = workspace("descendant-pipe");
        let executor =
            Executor::new(&root, ["sh".to_owned()], Duration::from_secs(10), 1024).unwrap();
        let started = Instant::now();
        // The shell exits immediately, but the backgrounded sleep inherits the
        // stdout/stderr pipes, so an unbounded reader join would wait for it.
        let result = executor
            .run(&root, "sh", &["-c", "sleep 5 & exit 0"])
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.status, Some(0));
        assert!(
            elapsed < Duration::from_secs(2),
            "run waited {elapsed:?} for a descendant that kept the pipe open"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_denies_writes_outside_workspace() {
        let root = workspace("macos-sandbox");
        let outside = std::env::temp_dir().join(format!(
            "agent-runtime-forbidden-write-{}",
            std::process::id()
        ));
        let executor = Executor::new(&root, ["sh".to_owned()], Duration::from_secs(2), 4096)
            .unwrap()
            .with_isolation(Isolation::MacOsSandbox)
            .unwrap();
        let script = format!("printf forbidden > {}", outside.display());
        let result = executor.run(&root, "sh", &["-c", &script]).unwrap();
        assert_ne!(result.status, Some(0));
        assert!(!outside.exists());

        let inside = root.join("allowed.txt");
        let script = format!("printf allowed > {}", inside.display());
        let result = executor.run(&root, "sh", &["-c", &script]).unwrap();
        assert_eq!(result.status, Some(0), "{}", result.stderr);
        assert_eq!(fs::read_to_string(inside).unwrap(), "allowed");
    }
}
