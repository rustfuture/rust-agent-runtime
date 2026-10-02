//! A scripted model, with outcomes taken from real worker runs and observations.
use rust_agent_runtime::{
    agent::AgentLoop,
    executor::{CancellationToken, Executor},
    provider::{mock::MockProvider, ModelAction},
    worker, Runtime, State,
};
use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const TEST: &str = "cargo test --offline --quiet --target-dir target";
const SOURCE: &str = "pub fn sum_up_to(n: u32) -> u32 { (1..n).sum() }\n\
    #[cfg(test)] mod tests {\n\
    #[test] fn includes_last_number() { assert_eq!(super::sum_up_to(5), 15); }\n\
    }\n";

struct TemporaryWorkspace(PathBuf);

impl TemporaryWorkspace {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = env::temp_dir().join(format!("rar-demo-{}-{nonce}", std::process::id()));
        // Never reuse an existing directory, even on a name collision.
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct Step {
    label: &'static str,
    verb: &'static str,
    target: &'static str,
    reason: Option<String>,
}

struct Walkthrough {
    steps: Vec<Step>,
    attempts: u32,
    changed_files: usize,
    tool_runs: usize,
}

fn require(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn walkthrough() -> io::Result<Walkthrough> {
    let temporary = TemporaryWorkspace::new()?;
    let workspace = temporary.0.join("workspace");
    fs::create_dir(&workspace)?;
    fs::create_dir(workspace.join("src"))?;
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"demo-sum\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
    )?;
    fs::write(workspace.join("src/lib.rs"), SOURCE)?;
    let executor = Executor::new(
        &workspace,
        ["cargo".to_owned()],
        Duration::from_secs(30),
        16 * 1024,
    )?;
    let agent = AgentLoop::new(
        6,
        vec!["cargo".to_owned()],
        vec![TEST.to_owned()],
        16 * 1024,
    )?
    .with_required_edit(true);
    let mut runtime = Runtime::open(&temporary.0.join("runtime"))?;
    let token = CancellationToken::default();
    let mut provider = MockProvider::from_actions([
        ModelAction::RunTool {
            program: "cargo".to_owned(),
            args: vec![
                "test".to_owned(),
                "--offline".to_owned(),
                "--quiet".to_owned(),
                "--target-dir".to_owned(),
                "target".to_owned(),
            ],
        },
        ModelAction::RunTool {
            program: "curl".to_owned(),
            args: vec!["--version".to_owned()],
        },
    ]);
    let blocked = worker::run_agent_task(
        &mut runtime,
        "sum-task",
        &agent,
        &mut provider,
        &executor,
        &workspace,
        "Fix sum_up_to so it includes the last number.",
        &token,
    );
    let blocked = match blocked {
        Err(error) => error,
        Ok(_) => {
            return Err(io::Error::other(
                "demo blocked command unexpectedly succeeded",
            ))
        }
    };
    require(
        blocked.kind() == io::ErrorKind::PermissionDenied
            && provider.requests().len() == 2
            && runtime.task("sum-task").map(|task| task.state) == Some(State::Failed),
        "demo expected the allowlist to fail the first attempt",
    )?;
    let first_trace = &runtime.tool_traces("sum-task")[0];
    require(
        first_trace.status.is_some_and(|status| status != 0)
            && !first_trace.timed_out
            && !first_trace.cancelled,
        "demo expected the initial tests to fail",
    )?;
    let failure = provider.requests()[1]
        .observations
        .last()
        .ok_or_else(|| io::Error::other("missing initial test observation"))?;
    let mut steps = vec![
        Step {
            label: "FAIL",
            verb: "test",
            target: TEST,
            reason: Some(first_sentence(
                failure
                    .lines()
                    .find(|line| line.starts_with("assertion "))
                    .unwrap_or(failure),
            )),
        },
        Step {
            label: "BLOCK",
            verb: "stop/retry",
            target: "curl --version",
            reason: Some(first_sentence(&blocked.to_string())),
        },
    ];
    // A denied command terminates the real worker. Retry the same durable task,
    // rather than hiding that boundary or changing the runtime for the demo.
    require(runtime.retry("sum-task", 2)?, "demo retry was refused")?;
    let mut provider = MockProvider::from_actions([
        ModelAction::ReplaceText {
            path: "src/lib.rs".to_owned(),
            expected: "(1..n)".to_owned(),
            replacement: "(1..=n)".to_owned(),
        },
        ModelAction::Finish {
            summary: "Fixed the range.".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "Fixed the range and verified the tests.".to_owned(),
        },
    ]);
    let report = worker::run_agent_task(
        &mut runtime,
        "sum-task",
        &agent,
        &mut provider,
        &executor,
        &workspace,
        "Fix sum_up_to so it includes the last number.",
        &token,
    )?;
    let requests = provider.requests();
    require(requests.len() == 4, "demo expected four decisions on retry")?;
    require(
        requests[1].observations.last().map(String::as_str)
            == Some("replaced exact text in file=src/lib.rs"),
        "demo source edit was not applied",
    )?;
    let rejection = requests[2]
        .observations
        .last()
        .filter(|observation| observation.starts_with("finish rejected:"))
        .ok_or_else(|| io::Error::other("demo early finish was not rejected"))?;
    let traces = runtime.tool_traces("sum-task");
    require(
        traces.len() == 2
            && traces[1].status == Some(0)
            && !traces[1].timed_out
            && !traces[1].cancelled
            && report.verified_after_change,
        "demo verification did not pass",
    )?;
    let task = runtime
        .task("sum-task")
        .ok_or_else(|| io::Error::other("missing demo task"))?;
    require(
        task.state == State::Succeeded,
        "demo finish was not accepted",
    )?;
    steps.extend([
        Step {
            label: "EDIT",
            verb: "replace",
            target: "src/lib.rs  ·  1..n → 1..=n",
            reason: None,
        },
        Step {
            label: "REJECT",
            verb: "finish",
            target: "verification still required",
            reason: Some(first_sentence(rejection)),
        },
        Step {
            label: "PASS",
            verb: "verify",
            target: TEST,
            reason: None,
        },
        Step {
            label: "DONE",
            verb: "finish",
            target: "sum-task",
            reason: None,
        },
    ]);
    Ok(Walkthrough {
        steps,
        attempts: task.attempts,
        changed_files: report.changed_files,
        tool_runs: traces.len(),
    })
}

fn first_sentence(reason: &str) -> String {
    let line = reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(reason);
    let mut chars = line.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(ch, '.' | '!' | '?')
            && chars.peek().is_none_or(|(_, next)| next.is_whitespace())
        {
            return line[..index + ch.len_utf8()].trim().to_owned();
        }
    }
    line.trim().to_owned()
}

fn styled(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn render(
    output: &mut impl Write,
    story: &Walkthrough,
    color: bool,
    animate: bool,
) -> io::Result<()> {
    writeln!(
        output,
        "\n  {}",
        styled(
            "╭──────────────────────────────────────────────────╮",
            "2",
            color
        )
    )?;
    writeln!(
        output,
        "  {} {}{} {}",
        styled("│", "2", color),
        styled("Rust Agent Runtime", "1", color),
        styled("  ·  one task, start to finish", "2", color),
        styled("│", "2", color)
    )?;
    writeln!(
        output,
        "  {}\n",
        styled(
            "╰──────────────────────────────────────────────────╯",
            "2",
            color
        )
    )?;
    for step in &story.steps {
        if animate {
            thread::sleep(Duration::from_millis(450));
        }
        let code = match step.label {
            "BLOCK" => "1;30;41",
            "REJECT" => "1;30;43",
            "FAIL" => "1;30;101",
            _ => "1;30;42",
        };
        writeln!(
            output,
            "  {}  {}  {}",
            styled(&format!(" {:<6}", step.label), code, color),
            styled(&format!("{:<10}", step.verb), "2", color),
            step.target
        )?;
        if let Some(reason) = &step.reason {
            writeln!(
                output,
                "           {}",
                styled(&format!("↳ {reason}"), "2", color)
            )?;
        }
        output.flush()?;
    }
    writeln!(output, "\n  {} decisions  ·  {} test runs  ·  {} file edited  ·  {} attempts (blocked attempt retried)", styled(&story.steps.len().to_string(), "1;32", color), styled(&story.tool_runs.to_string(), "1;32", color), styled(&story.changed_files.to_string(), "1;32", color), styled(&story.attempts.to_string(), "1;33", color))?;
    writeln!(
        output,
        "  {}",
        styled(
            "Decisions come from the real runtime; the model is scripted.",
            "2",
            color
        )
    )
}

pub fn run() -> io::Result<()> {
    let story = walkthrough()?;
    let terminal = io::stdout().is_terminal();
    let color = terminal && env::var_os("NO_COLOR").is_none();
    let animate = terminal && env::var_os("RAR_DEMO_FAST").is_none();
    render(&mut io::stdout().lock(), &story, color, animate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_keep_whole_words_and_only_the_first_sentence() {
        assert_eq!(
            first_sentence("Failed in src/lib.rs. More detail."),
            "Failed in src/lib.rs."
        );
        assert_eq!(
            first_sentence("First line without punctuation\nsecond line"),
            "First line without punctuation"
        );
    }

    #[test]
    fn six_real_runtime_outcomes() {
        let story = walkthrough().unwrap();
        assert_eq!(
            story
                .steps
                .iter()
                .map(|step| step.label)
                .collect::<Vec<_>>(),
            ["FAIL", "BLOCK", "EDIT", "REJECT", "PASS", "DONE"]
        );
        assert_eq!(story.attempts, 2);
        assert_eq!(story.changed_files, 1);
        assert_eq!(story.tool_runs, 2);
        assert_eq!(
            story.steps[1].reason.as_deref(),
            Some("program is not allowlisted")
        );
        assert_eq!(
            story.steps[3].reason.as_deref(),
            Some("finish rejected: run a successful verification command after the latest edit")
        );
        let mut screen = Vec::new();
        render(&mut screen, &story, false, false).unwrap();
        let screen = String::from_utf8(screen).unwrap();
        assert!(!screen.contains('\x1b'));
        for step in &story.steps {
            assert!(screen.contains(step.label));
        }
    }
}
