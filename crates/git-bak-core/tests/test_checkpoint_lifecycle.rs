use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use git_bak_core::{
    GitExecutor, GitRepo, PersonaWatcher, SharedSystemState, SystemState, WatchMode,
    WorkspaceConfig, checkpoint,
};
use tempfile::tempdir;

#[test]
fn test_checkpoint_lifecycle() {
    let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
    let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));

    fs::write(dir.path().join("SOUL.md"), "seed\n")
        .unwrap_or_else(|err| panic!("write seed failed: {err}"));
    repo.add(Path::new("SOUL.md"))
        .unwrap_or_else(|err| panic!("add seed failed: {err}"));
    repo.commit("seed")
        .unwrap_or_else(|err| panic!("seed commit failed: {err}"));
    repo.run_git_args(["checkout", "-b", "raw"])
        .unwrap_or_else(|err| panic!("create raw branch failed: {err}"));
    fs::write(dir.path().join("SOUL.md"), "raw-initial\n")
        .unwrap_or_else(|err| panic!("write raw initial failed: {err}"));
    repo.add(Path::new("SOUL.md"))
        .unwrap_or_else(|err| panic!("add raw initial failed: {err}"));
    repo.commit("raw initial")
        .unwrap_or_else(|err| panic!("raw initial commit failed: {err}"));

    let config = WorkspaceConfig {
        workspace: dir.path().to_path_buf(),
        watch: vec!["SOUL.md".to_owned()],
        debounce_ms: 50,
        push_interval_sec: 600,
        push_commit_threshold: 50,
        push_backoff_base_sec: 30,
        storm_window_sec: 1,
        hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
        mode: WatchMode::Watcher,
    };

    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();
    let watcher = PersonaWatcher::new(&config, executor.sender(), state.clone())
        .unwrap_or_else(|err| panic!("watcher init failed: {err}"));
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());

    let result = checkpoint(&executor.sender(), &state, "2026-02-18T00:00:00Z");
    assert!(result.is_ok());
    assert_eq!(
        state.current().unwrap_or(SystemState::Paused),
        SystemState::Running
    );
    let checkpoint_message = repo
        .run_git_args(["log", "-1", "--pretty=%s", "checkpoint"])
        .unwrap_or_else(|err| panic!("failed to read checkpoint log: {err}"));
    assert!(checkpoint_message.starts_with("checkpoint: "));

    let pre_resume_count = git_branch_commit_count(repo.path(), "raw");
    fs::write(dir.path().join("SOUL.md"), "after-checkpoint\n")
        .unwrap_or_else(|err| panic!("write after checkpoint failed: {err}"));
    let post_count = wait_for_branch_commits(
        repo.path(),
        "raw",
        pre_resume_count + 1,
        Duration::from_secs(5),
    );
    assert!(post_count >= pre_resume_count + 1);

    stop_handle.store(true, Ordering::Relaxed);
    let join_result = worker.join();
    assert!(join_result.is_ok());
    let run_result = join_result.unwrap_or_else(|_| panic!("watcher thread panicked"));
    assert!(run_result.is_ok());
    assert!(executor.stop().is_ok());
}

fn wait_for_branch_commits(
    repo_path: &Path,
    branch: &str,
    expected: i64,
    timeout: Duration,
) -> i64 {
    let start = Instant::now();
    let mut last = 0;
    while start.elapsed() <= timeout {
        last = git_branch_commit_count(repo_path, branch);
        if last >= expected {
            return last;
        }
        thread::sleep(Duration::from_millis(100));
    }
    last
}

fn git_branch_commit_count(repo_path: &Path, branch: &str) -> i64 {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-list", "--count", branch])
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
