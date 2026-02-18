use std::fs;
use std::path::Path;
use std::process::Command;

use git_bak_core::{GitExecutor, GitRepo, HookHandler, WatchMode, WorkspaceConfig};
use tempfile::tempdir;

#[test]
fn test_hook_executor() {
    let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
    let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
    let config = WorkspaceConfig {
        workspace: dir.path().to_path_buf(),
        watch: vec!["SOUL.md".to_owned()],
        debounce_ms: 50,
        push_interval_sec: 600,
        push_commit_threshold: 50,
        push_backoff_base_sec: 30,
        storm_window_sec: 5,
        hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
        mode: WatchMode::Hook,
    };

    let mut executor = GitExecutor::start(repo.clone());
    let handler = HookHandler::new(&config, executor.sender());
    assert!(handler.ensure_signal_dir().is_ok());

    fs::write(dir.path().join("SOUL.md"), "hook signal test\n")
        .unwrap_or_else(|err| panic!("write watched file failed: {err}"));
    let signal_path = handler.signal_dir().join("signal-01.json");
    fs::write(
        &signal_path,
        r#"{"event":"command:stop","dedupe_key":"01HOOKEXEC","timestamp":"2026-02-18T00:00:00Z"}"#,
    )
    .unwrap_or_else(|err| panic!("write signal file failed: {err}"));

    let processed = handler
        .handle_signal_file(signal_path.as_path())
        .unwrap_or_else(|err| panic!("handle signal failed: {err}"));
    assert!(processed);
    assert!(git_commit_count(repo.path()) >= 1);
    assert!(executor.stop().is_ok());
}

fn git_commit_count(repo_path: &Path) -> i64 {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-list", "--count", "HEAD"])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(_) => return 0,
    };
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i64>()
        .unwrap_or(0)
}
