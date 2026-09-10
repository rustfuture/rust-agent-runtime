use rust_agent_runtime::{
    agent::AgentLoop,
    executor::{CancellationToken, Executor, Isolation},
    provider::AgyProvider,
    worker, Runtime, State,
};
use std::{
    env, io,
    path::Path,
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [command, data_dir, id] if command == "enqueue" => {
            let mut runtime = Runtime::open(Path::new(data_dir))?;
            println!("enqueued={}", runtime.enqueue(id)?);
        }
        [command, data_dir, id] if command == "cancel" => {
            // Inspect rather than open: a cancel must not trigger restart
            // recovery (which could requeue a live task) as a side effect.
            let mut runtime = Runtime::inspect(Path::new(data_dir))?;
            runtime.cancel(id)?;
            println!("cancelled={id}");
        }
        [command, data_dir, id, workspace] if command == "run" => {
            run_agent_worker(data_dir, id, workspace)?;
        }
        [command, data_dir] if command == "status" => render(Path::new(data_dir))?,
        [command, data_dir, interval] if command == "watch" => {
            let interval: u64 = interval.parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "interval must be milliseconds")
            })?;
            if interval == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "interval must be positive",
                ));
            }
            loop {
                print!("\x1b[2J\x1b[H");
                render(Path::new(data_dir))?;
                thread::sleep(Duration::from_millis(interval));
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: rust-agent-runtime <enqueue DATA_DIR ID | cancel DATA_DIR ID | run DATA_DIR ID WORKSPACE | status DATA_DIR | watch DATA_DIR INTERVAL_MS>",
            ));
        }
    }
    Ok(())
}

fn env_list(name: &str, separator: char) -> Vec<String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .split(separator)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> io::Result<T> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{name} is invalid"))),
        Err(_) => Ok(default),
    }
}

fn run_agent_worker(data_dir: &str, id: &str, workspace: &str) -> io::Result<()> {
    let task = env::var("AGENT_TASK").map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "AGENT_TASK is required for the run command",
        )
    })?;
    let allowed = env_list("AGENT_ALLOWED", ',');
    if allowed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AGENT_ALLOWED must list at least one program",
        ));
    }
    let verification = env_list("AGENT_VERIFY", ';');
    let max_steps = env_parse("AGENT_MAX_STEPS", 8usize)?;
    let timeout_secs = env_parse("AGENT_TIMEOUT_SECS", 120u64)?;
    let max_observation = env_parse("AGENT_MAX_OBSERVATION_BYTES", 16 * 1024usize)?;
    let max_output = env_parse("AGENT_MAX_OUTPUT_BYTES", 64 * 1024usize)?;
    let binary = env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_owned());
    let provider_dir = env::var("AGY_WORK_DIR").unwrap_or_else(|_| workspace.to_owned());
    let model = env::var("AGY_MODEL").unwrap_or_else(|_| "gemini-3.8-flash-low".to_owned());

    let mut provider = AgyProvider::new(
        Path::new(&binary),
        Path::new(&provider_dir),
        model,
        Duration::from_secs(timeout_secs),
    )?
    .with_output_limit(max_output)?;
    if let Some(script) = env::var_os("AGENT_FAKE_PROVIDER").filter(|value| !value.is_empty()) {
        // Harness-only deterministic provider: a local script emits AGY-shaped
        // envelopes through the same supervised process path.
        provider = provider.with_fake_script(Path::new(&script))?;
    }
    let executor = Executor::new(
        Path::new(workspace),
        allowed.clone(),
        Duration::from_secs(timeout_secs),
        max_output,
    )?;
    #[cfg(target_os = "macos")]
    let executor = executor.with_isolation(Isolation::MacOsSandbox)?;
    #[cfg(not(target_os = "macos"))]
    let executor = {
        let _ = Isolation::None;
        executor
    };
    let agent =
        AgentLoop::new(max_steps, allowed, verification, max_observation)?.with_required_edit(true);
    let mut runtime = Runtime::open(Path::new(data_dir))?;

    // Poll the durable log so `cancel DATA_DIR ID` from another process reaches
    // the live worker through the shared token.
    let cancellation = CancellationToken::default();
    let monitor_done = Arc::new(AtomicBool::new(false));
    let monitor_flag = Arc::clone(&monitor_done);
    let monitor_token = cancellation.clone();
    let monitor_dir = data_dir.to_owned();
    let monitor_id = id.to_owned();
    let monitor = thread::spawn(move || {
        while !monitor_flag.load(Ordering::SeqCst) {
            if let Ok(inspected) = Runtime::inspect(Path::new(&monitor_dir)) {
                if inspected.task(&monitor_id).map(|task| task.state) == Some(State::Cancelled) {
                    monitor_token.cancel();
                    return;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    let result = worker::run_agent_task(
        &mut runtime,
        id,
        &agent,
        &mut provider,
        &executor,
        Path::new(workspace),
        &task,
        &cancellation,
    );
    monitor_done.store(true, Ordering::SeqCst);
    let _ = monitor.join();

    match result {
        Ok(report) => {
            println!("outcome=completed");
            println!("summary={}", report.summary);
            println!("steps={}", report.decisions.len());
            println!("tool_runs={}", report.tool_runs);
            println!("changed_files={}", report.changed_files);
            println!("verified_after_change={}", report.verified_after_change);
            println!(
                "input_tokens={}",
                report
                    .decisions
                    .iter()
                    .filter_map(|decision| decision.input_tokens)
                    .sum::<u64>()
            );
            println!(
                "output_tokens={}",
                report
                    .decisions
                    .iter()
                    .filter_map(|decision| decision.output_tokens)
                    .sum::<u64>()
            );
            println!(
                "provider_duration_ms={}",
                report
                    .decisions
                    .iter()
                    .map(|decision| decision.duration_ms)
                    .sum::<u128>()
            );
            Ok(())
        }
        Err(error) => {
            println!("outcome=failed");
            println!("failure_kind={:?}", error.kind());
            println!("failure={error}");
            Err(error)
        }
    }
}

fn render(data_dir: &Path) -> io::Result<()> {
    let runtime = Runtime::inspect(data_dir)?;
    println!("TASK\tSTATE\tATTEMPTS\tLAST_TOOL\tLAST_MS");
    for task in runtime.tasks() {
        let task_trace = runtime.execution_traces(&task.id).last();
        let tool_trace = runtime.tool_traces(&task.id).last();
        let trace = tool_trace.or(task_trace);
        println!(
            "{}\t{:?}\t{}\t{}\t{}",
            task.id,
            task.state,
            task.attempts,
            trace.map(|item| item.program.as_str()).unwrap_or("-"),
            trace
                .map(|item| item.duration_ms.to_string())
                .unwrap_or_else(|| "-".to_owned())
        );
    }
    Ok(())
}
