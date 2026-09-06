use rust_agent_runtime::{
    agent::AgentLoop,
    executor::{CancellationToken, Executor},
    provider::AgyProvider,
};
use std::{env, io, path::Path, time::Duration};

fn main() -> io::Result<()> {
    let binary = env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_owned());
    let provider_dir = env::var("AGY_WORK_DIR").unwrap_or_else(|_| ".".to_owned());
    let fixture = env::var("FIXTURE_DIR")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "FIXTURE_DIR is required"))?;
    let mut provider = AgyProvider::new(
        Path::new(&binary),
        Path::new(&provider_dir),
        "gemini-3.8-flash-low",
        Duration::from_secs(30),
    )?;
    let executor = Executor::new(
        Path::new(&fixture),
        ["cargo".to_owned()],
        Duration::from_secs(30),
        16 * 1024,
    )?;
    let agent = AgentLoop::new(8, vec!["cargo".to_owned()], 16 * 1024)?;
    let report = agent.run(
        &mut provider,
        &executor,
        Path::new(&fixture),
        "Fix the off-by-one defect in this small Rust crate. Make the smallest correct source change and verify it with cargo test.",
        &CancellationToken::default(),
    )?;
    println!("summary={}", report.summary);
    println!("steps={}", report.decisions.len());
    println!("tool_runs={}", report.tool_runs);
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
