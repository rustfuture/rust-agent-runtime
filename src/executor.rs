use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
pub struct Execution {
    pub status: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub duration_ms: u128,
}

pub struct Executor {
    workspace: PathBuf,
    allowed: HashSet<String>,
    timeout: Duration,
    max_output_bytes: usize,
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
        })
    }

    pub fn run(&self, cwd: &Path, program: &str, args: &[&str]) -> io::Result<Execution> {
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
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stdout_path = std::env::temp_dir().join(format!("agent-runtime-{nonce}.stdout"));
        let stderr_path = std::env::temp_dir().join(format!("agent-runtime-{nonce}.stderr"));
        let stdout_file = File::create(&stdout_path)?;
        let stderr_file = File::create(&stderr_path)?;
        let started = Instant::now();
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(stdout_file)
            .stderr(stderr_file)
            .spawn()?;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status.code();
            }
            if started.elapsed() >= self.timeout {
                timed_out = true;
                child.kill()?;
                break child.wait()?.code();
            }
            thread::sleep(Duration::from_millis(10));
        };
        let (stdout, stdout_truncated) = read_limited(&stdout_path, self.max_output_bytes)?;
        let (stderr, stderr_truncated) = read_limited(&stderr_path, self.max_output_bytes)?;
        let _ = fs::remove_file(stdout_path);
        let _ = fs::remove_file(stderr_path);
        Ok(Execution {
            status,
            timed_out,
            stdout,
            stderr,
            output_truncated: stdout_truncated || stderr_truncated,
            duration_ms: started.elapsed().as_millis(),
        })
    }
}

fn read_limited(path: &Path, limit: usize) -> io::Result<(String, bool)> {
    let file = File::open(path)?;
    let length = file.metadata()?.len() as usize;
    let mut bytes = Vec::new();
    file.take(limit as u64).read_to_end(&mut bytes)?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), length > limit))
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
}
