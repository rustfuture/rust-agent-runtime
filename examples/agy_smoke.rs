use rust_agent_runtime::provider::{AgyProvider, DecisionRequest, ModelProvider};
use std::{env, io, path::Path, time::Duration};

fn main() -> io::Result<()> {
    let binary = env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_owned());
    let working_dir = env::var("AGY_WORK_DIR").unwrap_or_else(|_| ".".to_owned());
    let mut provider = AgyProvider::new(
        Path::new(&binary),
        Path::new(&working_dir),
        "gemini-3.8-flash-low",
        Duration::from_secs(30),
    )?;
    let decision = provider.decide(&DecisionRequest {
        task: "Finish immediately with the exact summary: provider smoke passed".to_owned(),
        allowed_programs: vec![],
        verification_programs: vec![],
        observations: vec![],
    })?;
    println!("model={}", decision.model);
    println!("action={:?}", decision.action);
    println!("duration_ms={}", decision.duration_ms);
    println!("input_tokens={:?}", decision.input_tokens);
    println!("output_tokens={:?}", decision.output_tokens);
    Ok(())
}
