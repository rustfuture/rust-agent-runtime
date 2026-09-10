use std::{fs, path::PathBuf, process::Command};

fn temp(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "cli-fake-provider-{}-{name}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn fake_provider_tool_still_passes_through_the_executor_allowlist() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp("allowlist");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("note.txt"), "keep").unwrap();
    let script = root.join("fake-provider.sh");
    fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' '{\"status\":\"SUCCESS\",\"structured_output\":{\"kind\":\"run_tool\",\"program\":\"rm\",\"args\":[\"-rf\",\".\"]}}'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let data = root.join("data");
    let output = Command::new(env!("CARGO_BIN_EXE_rust-agent-runtime"))
        .args([
            "run",
            data.to_str().unwrap(),
            "allowlist-task",
            workspace.to_str().unwrap(),
        ])
        .env("AGENT_TASK", "delete everything")
        .env("AGENT_ALLOWED", "true")
        .env("AGENT_VERIFY", "true")
        .env("AGENT_MAX_STEPS", "1")
        .env("AGENT_FAKE_PROVIDER", &script)
        .output()
        .unwrap();

    assert!(!output.status.success(), "the worker must fail the task");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PermissionDenied"),
        "expected an allowlist denial, got: {stdout}"
    );
    let events = fs::read_to_string(data.join("events.log")).unwrap();
    assert!(events.contains("\tfailed\t"), "events: {events}");
    assert!(
        workspace.join("note.txt").exists(),
        "workspace was modified"
    );
    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn fake_provider_enforces_the_local_provider_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp("timeout");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let script = root.join("fake-provider.sh");
    fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let data = root.join("data");
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_rust-agent-runtime"))
        .args([
            "run",
            data.to_str().unwrap(),
            "timeout-task",
            workspace.to_str().unwrap(),
        ])
        .env("AGENT_TASK", "stall")
        .env("AGENT_ALLOWED", "true")
        .env("AGENT_VERIFY", "true")
        .env("AGENT_MAX_STEPS", "1")
        .env("AGENT_TIMEOUT_SECS", "1")
        .env("AGENT_FAKE_PROVIDER", &script)
        .output()
        .unwrap();

    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("TimedOut"),
        "expected a local timeout, got: {stdout}"
    );
    let events = fs::read_to_string(data.join("events.log")).unwrap();
    assert!(events.contains("\tfailed\t"), "events: {events}");
    let _ = fs::remove_dir_all(&root);
}
