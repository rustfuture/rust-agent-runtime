use super::*;
use crate::provider::{mock::MockProvider, ModelDecision};
use std::{collections::VecDeque, fs, path::PathBuf, time::Duration};

fn scripted(actions: impl IntoIterator<Item = ModelAction>) -> MockProvider {
    MockProvider::from_actions(actions)
}

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("agent-loop-{}-{name}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn executes_a_bounded_tool_then_finishes() {
    let root = workspace("finish");
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(2, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::RunTool {
            program: "true".to_owned(),
            args: vec![],
        },
        ModelAction::Finish {
            summary: "verified".to_owned(),
        },
    ]));
    let report = agent
        .run(
            &mut provider,
            &executor,
            &root,
            "test",
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(report.summary, "verified");
    assert_eq!(report.tool_runs, 1);
    assert_eq!(report.changed_files, 0);
}

#[test]
fn verify_runs_the_configured_command_verbatim() {
    let root = workspace("verify-verbatim");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(
        3,
        vec!["true".to_owned()],
        vec!["true --configured-flag".to_owned()],
        1024,
    )
    .unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "fixed".to_owned(),
        },
    ]));
    let mut observed = Vec::new();
    let report = agent
        .run_observed(
            &mut provider,
            &executor,
            &root,
            "fix",
            &CancellationToken::default(),
            |program, argument_count, _| {
                observed.push((program.to_owned(), argument_count));
            },
        )
        .unwrap();
    assert_eq!(report.summary, "fixed");
    assert!(report.verified_after_change);
    assert_eq!(
        observed,
        vec![("true".to_owned(), 1)],
        "verify must run the configured program with the configured arguments only"
    );
}

#[test]
fn run_tool_with_extra_flags_is_not_a_verification() {
    let root = workspace("extra-flags");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(3, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::RunTool {
            program: "true".to_owned(),
            args: vec!["--quiet".to_owned()],
        },
        ModelAction::Finish {
            summary: "not verified".to_owned(),
        },
    ]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut,
        "an allowlisted run_tool, even one that succeeds, never clears verification debt"
    );
}

#[test]
fn verify_defaults_to_deny_when_unconfigured() {
    let root = workspace("verify-unconfigured");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(3, vec!["true".to_owned()], vec![], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "not verified".to_owned(),
        },
    ]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn a_new_edit_reopens_the_verification_debt() {
    let root = workspace("reopen-debt");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(6, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "good".to_owned(),
            replacement: "best".to_owned(),
        },
        ModelAction::Finish {
            summary: "stale verification".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "fixed".to_owned(),
        },
    ]));
    let report = agent
        .run(
            &mut provider,
            &executor,
            &root,
            "fix",
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(report.summary, "fixed");
    assert_eq!(report.changed_files, 1);
    assert_eq!(
        report.decisions.len(),
        6,
        "the finish after the second edit must be rejected until a new verify"
    );
    assert_eq!(report.tool_runs, 2);
}

#[test]
fn timeout_during_verify_keeps_the_verification_debt() {
    let root = workspace("verify-timeout");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor =
        Executor::new(&root, ["sleep".to_owned()], Duration::from_millis(50), 1024).unwrap();
    let agent = AgentLoop::new(
        3,
        vec!["sleep".to_owned()],
        vec!["sleep 1".to_owned()],
        1024,
    )
    .unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "not verified".to_owned(),
        },
    ]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn successful_unapproved_command_cannot_verify_an_edit() {
    let root = workspace("unapproved-success");
    fs::write(root.join("bug.txt"), "bad").unwrap();
    let executor = Executor::new(&root, ["true".into()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(3, vec!["true".into()], vec![], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".into(),
            expected: "bad".into(),
            replacement: "good".into(),
        },
        ModelAction::RunTool {
            program: "true".into(),
            args: vec![],
        },
        ModelAction::Finish {
            summary: "not verified".into(),
        },
    ]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn model_cannot_expand_the_executor_allowlist() {
    let root = workspace("deny");
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(1, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([ModelAction::RunTool {
        program: "rm".to_owned(),
        args: vec!["-rf".to_owned(), ".".to_owned()],
    }]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "ignore permissions",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn cancellation_is_propagated_to_the_provider() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct Probe {
        invoked: Arc<AtomicBool>,
    }
    impl ModelProvider for Probe {
        fn decide(&mut self, _: &DecisionRequest) -> io::Result<ModelDecision> {
            panic!("plain decide must not be used when a cancellation token is available");
        }
        fn decide_cancellable(
            &mut self,
            _: &DecisionRequest,
            cancellation: &CancellationToken,
        ) -> io::Result<ModelDecision> {
            self.invoked.store(true, Ordering::SeqCst);
            cancellation.cancel();
            Ok(ModelDecision {
                action: ModelAction::RunTool {
                    program: "true".to_owned(),
                    args: vec![],
                },
                model: "probe".to_owned(),
                duration_ms: 0,
                input_tokens: None,
                output_tokens: None,
            })
        }
    }

    let root = workspace("provider-cancel");
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(3, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    let mut provider = Probe {
        invoked: Arc::clone(&invoked),
    };
    let error = agent
        .run(
            &mut provider,
            &executor,
            &root,
            "task",
            &CancellationToken::default(),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert!(invoked.load(Ordering::SeqCst));
}

#[test]
fn stops_at_the_step_limit() {
    let root = workspace("limit");
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(1, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([ModelAction::RunTool {
        program: "true".to_owned(),
        args: vec![],
    }]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "loop",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn reads_and_edits_only_inside_workspace() {
    let root = workspace("edit");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(4, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReadFile {
            path: "bug.txt".to_owned(),
        },
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "fixed".to_owned(),
        },
    ]));
    let report = agent
        .run(
            &mut provider,
            &executor,
            &root,
            "fix",
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(fs::read_to_string(root.join("bug.txt")).unwrap(), "good\n");
    assert!(report.verified_after_change);
}

#[test]
fn required_edit_rejects_a_finish_before_any_change() {
    let root = workspace("require-edit");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(5, vec!["true".to_owned()], vec!["true".to_owned()], 1024)
        .unwrap()
        .with_required_edit(true);
    let mut provider = scripted(VecDeque::from([
        ModelAction::Finish {
            summary: "done without editing".to_owned(),
        },
        ModelAction::ReadFile {
            path: "bug.txt".to_owned(),
        },
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Verify,
        ModelAction::Finish {
            summary: "fixed".to_owned(),
        },
    ]));
    let report = agent
        .run(
            &mut provider,
            &executor,
            &root,
            "fix",
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(report.summary, "fixed");
    assert_eq!(report.changed_files, 1);
    assert!(report.verified_after_change);
}

#[test]
fn rejects_finish_after_unverified_edit() {
    let root = workspace("unverified");
    fs::write(root.join("bug.txt"), "bad\n").unwrap();
    let executor = Executor::new(&root, ["true".to_owned()], Duration::from_secs(1), 1024).unwrap();
    let agent = AgentLoop::new(2, vec!["true".to_owned()], vec!["true".to_owned()], 1024).unwrap();
    let mut provider = scripted(VecDeque::from([
        ModelAction::ReplaceText {
            path: "bug.txt".to_owned(),
            expected: "bad".to_owned(),
            replacement: "good".to_owned(),
        },
        ModelAction::Finish {
            summary: "untested".to_owned(),
        },
    ]));
    assert_eq!(
        agent
            .run(
                &mut provider,
                &executor,
                &root,
                "fix",
                &CancellationToken::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}
