use rust_agent_runtime::{
    agent::AgentLoop,
    executor::{CancellationToken, Executor, Isolation},
    provider::AgyProvider,
};
use std::{env, io, path::Path, time::Duration};

fn main() -> io::Result<()> {
    let binary = env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_owned());
    let provider_dir = env::var("AGY_WORK_DIR").unwrap_or_else(|_| ".".to_owned());
    let fixture = env::var("FIXTURE_DIR")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "FIXTURE_DIR is required"))?;
    let timeout_secs = env::var("TIMEOUT_SECS")
        .unwrap_or_else(|_| "60".to_owned())
        .parse()
        .unwrap_or(60);
    let mut provider = AgyProvider::new(
        Path::new(&binary),
        Path::new(&provider_dir),
        "gemini-3.8-flash-low",
        Duration::from_secs(timeout_secs),
    )?;
    let executor = Executor::new(
        Path::new(&fixture),
        ["cargo".to_owned()],
        Duration::from_secs(timeout_secs),
        16 * 1024,
    )?;
    #[cfg(target_os = "macos")]
    let executor = executor.with_isolation(Isolation::MacOsSandbox)?;
    #[cfg(not(target_os = "macos"))]
    let executor = {
        let _ = Isolation::None;
        executor
    };
    let max_steps = env::var("MAX_STEPS")
        .unwrap_or_else(|_| "8".to_owned())
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "MAX_STEPS must be an integer"))?;
    let agent = AgentLoop::new(
        max_steps,
        vec!["cargo".to_owned()],
        vec!["cargo test".to_owned()],
        16 * 1024,
    )?;
    let task = env::var("TASK").unwrap_or_else(|_| {
        "Fix the off-by-one defect in this small Rust crate. Make the smallest correct source change and verify it with cargo test.".to_owned()
    });
    let report = match agent.run(
        &mut provider,
        &executor,
        Path::new(&fixture),
        &task,
        &CancellationToken::default(),
    ) {
        Ok(report) => report,
        Err(error) => {
            println!("outcome=failed");
            println!("failure_kind={:?}", error.kind());
            println!("failure={error}");
            return Err(error);
        }
    };
    println!("outcome=completed");
    println!("summary={}", report.summary);
    println!("steps={}", report.decisions.len());
    println!("tool_runs={}", report.tool_runs);
    println!("changed_files={}", report.changed_files);
    println!("verified_after_change={}", report.verified_after_change);
    for (i, d) in report.decisions.iter().enumerate() {
        println!("decision[{}]={:?}", i, d.action);
    }
    println!(
        "total_input_tokens={}",
        report
            .decisions
            .iter()
            .filter_map(|item| item.input_tokens)
            .sum::<u64>()
    );
    println!(
        "total_output_tokens={}",
        report
            .decisions
            .iter()
            .filter_map(|item| item.output_tokens)
            .sum::<u64>()
    );
    println!(
        "total_provider_duration_ms={}",
        report
            .decisions
            .iter()
            .map(|item| item.duration_ms)
            .sum::<u128>()
    );
    Ok(())
}
