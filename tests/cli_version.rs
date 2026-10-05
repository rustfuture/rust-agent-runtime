use std::process::Command;

#[test]
fn version_flag_prints_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_rust-agent-runtime"))
        .arg("--version")
        .output()
        .expect("run CLI");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("rust-agent-runtime {}", env!("CARGO_PKG_VERSION"))
    );
}
