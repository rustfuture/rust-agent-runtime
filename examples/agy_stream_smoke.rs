use rust_agent_runtime::provider::AgyProvider;
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
    let result = provider.stream_text(
        "Reply with exactly STREAM_OK and do not use tools.",
        |delta| print!("{delta}"),
    )?;
    println!("duration_ms={}", result.duration_ms);
    println!("input_tokens={:?}", result.input_tokens);
    println!("output_tokens={:?}", result.output_tokens);
    Ok(())
}
