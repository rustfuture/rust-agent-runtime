use std::{fs, 
    collections::HashSet,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

static NEXT_EXECUTION: AtomicU64 = AtomicU64::new(0);

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
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout_handle = child.stdout.take().unwrap();
        let mut stderr_handle = child.stderr.take().unwrap();
        
        let max_bytes = self.max_output_bytes as u64;
        let stdout_thread = thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = vec![0; 4096];
            let mut truncated = false;
            while buffer.len() < max_bytes as usize {
                let to_read = std::cmp::min(4096, max_bytes as usize - buffer.len());
                match stdout_handle.read(&mut chunk[..to_read]) {
                    Ok(0) => break,
                    Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            // Check if there's more available without blocking? 
            // Actually, if we just drop stdout_handle, the child gets SIGPIPE.
            // But let's check if it's truncated by reading one more byte.
            if buffer.len() == max_bytes as usize {
                let mut tiny = [0; 1];
                if let Ok(1) = stdout_handle.read(&mut tiny) {
                    truncated = true;
                }
            }
            drop(stdout_handle);
            (String::from_utf8_lossy(&buffer).into_owned(), truncated)
        });

        let stderr_thread = thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = vec![0; 4096];
            let mut truncated = false;
            while buffer.len() < max_bytes as usize {
                let to_read = std::cmp::min(4096, max_bytes as usize - buffer.len());
                match stderr_handle.read(&mut chunk[..to_read]) {
                    Ok(0) => break,
                    Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            if buffer.len() == max_bytes as usize {
                let mut tiny = [0; 1];
                if let Ok(1) = stderr_handle.read(&mut tiny) {
                    truncated = true;
                }
            }
            drop(stderr_handle);
            (String::from_utf8_lossy(&buffer).into_owned(), truncated)
        });
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
        let (stdout, stdout_truncated) = stdout_thread.join().unwrap();
        let (stderr, stderr_truncated) = stderr_thread.join().unwrap();
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

fn terminate_child(child: &mut std::process::Child) -> io::Result<()> {
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
