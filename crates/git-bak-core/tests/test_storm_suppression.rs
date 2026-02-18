use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use git_bak_core::{
    GitExecutor, GitRepo, PersonaWatcher, SharedSystemState, WatchMode, WorkspaceConfig,
};
use tempfile::tempdir;

#[test]
fn test_storm_suppression() {
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
        mode: WatchMode::Watcher,
    };

    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();
    let watcher = PersonaWatcher::new(&config, executor.sender(), state)
        .unwrap_or_else(|err| panic!("watcher init failed: {err}"));
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());
    thread::sleep(Duration::from_millis(300));

    fs::write(dir.path().join("SOUL.md"), "storm-initial\n")
        .unwrap_or_else(|err| panic!("write initial storm content failed: {err}"));
    let first_commit_count = wait_for_commits(repo.path(), 1, Duration::from_secs(6));
    assert!(first_commit_count >= 1);

    for i in 0..10 {
        fs::write(dir.path().join("SOUL.md"), format!("storm-{i}\n"))
            .unwrap_or_else(|err| panic!("write storm content failed: {err}"));
        thread::sleep(Duration::from_millis(50));
    }

    let commits = wait_for_commits(repo.path(), 3, Duration::from_secs(6));
    stop_handle.store(true, Ordering::Relaxed);
    let join_result = worker.join();
    assert!(join_result.is_ok());
    let run_result = join_result.unwrap_or_else(|_| panic!("watcher thread panicked"));
    assert!(run_result.is_ok());
    assert!(commits >= 1);
    assert!(commits <= 2);
    assert!(executor.stop().is_ok());
}

fn wait_for_commits(repo_path: &Path, max_expected: i64, timeout: Duration) -> i64 {
    let start = Instant::now();
    let mut last = 0;
    while start.elapsed() <= timeout {
        last = git_commit_count(repo_path);
        if last >= max_expected {
            return last;
        }
        thread::sleep(Duration::from_millis(100));
    }
    last
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
