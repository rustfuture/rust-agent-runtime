use rust_agent_runtime::Runtime;
use std::{env, io, path::Path, process::ExitCode, thread, time::Duration};

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
            let mut runtime = Runtime::open(Path::new(data_dir))?;
            runtime.cancel(id)?;
            println!("cancelled={id}");
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
                "usage: rust-agent-runtime <enqueue DATA_DIR ID | cancel DATA_DIR ID | status DATA_DIR | watch DATA_DIR INTERVAL_MS>",
            ));
        }
    }
    Ok(())
}

fn render(data_dir: &Path) -> io::Result<()> {
    let runtime = Runtime::inspect(data_dir)?;
    println!("TASK\tSTATE\tATTEMPTS\tLAST_TOOL\tLAST_MS");
    for task in runtime.tasks() {
        let trace = runtime.execution_traces(&task.id).last();
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
