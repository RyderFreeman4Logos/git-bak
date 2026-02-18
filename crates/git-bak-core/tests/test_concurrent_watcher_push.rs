use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use git_bak_core::{
    GitExecutor, GitRepo, PersonaWatcher, PushScheduler, SharedSystemState, WatchMode,
    WorkspaceConfig,
};
use tempfile::tempdir;

#[test]
fn test_concurrent_watcher_push() {
    let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
    let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
    let remote_path = dir.path().join("remote.git");
    init_bare_remote(&remote_path);
    set_origin_remote(repo.path(), &remote_path);

    let config = WorkspaceConfig {
        workspace: dir.path().to_path_buf(),
        watch: vec!["SOUL.md".to_owned()],
        debounce_ms: 50,
        push_interval_sec: 1,
        push_commit_threshold: 2,
        push_backoff_base_sec: 1,
        storm_window_sec: 0,
        hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
        mode: WatchMode::Watcher,
    };

    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();
    let mut scheduler = PushScheduler::start(&config, executor.sender(), executor.commit_count());
    let watcher = PersonaWatcher::new(&config, executor.sender(), state)
        .unwrap_or_else(|err| panic!("watcher init failed: {err}"));
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());
    thread::sleep(Duration::from_millis(300));

    for i in 0..6 {
        fs::write(dir.path().join("SOUL.md"), format!("event-{i}\n"))
            .unwrap_or_else(|err| panic!("write watched file failed: {err}"));
        thread::sleep(Duration::from_millis(250));
    }

    let pushed = wait_for_remote_commits(&remote_path, 1, Duration::from_secs(8));
    stop_handle.store(true, Ordering::Relaxed);
    let join_result = worker.join();
    assert!(join_result.is_ok());
    let run_result = join_result.unwrap_or_else(|_| panic!("watcher thread panicked"));
    assert!(run_result.is_ok());
    assert!(pushed);
    assert!(scheduler.stop().is_ok());
    assert!(executor.stop().is_ok());
}

fn init_bare_remote(path: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(path)
        .status()
        .unwrap_or_else(|err| panic!("failed to init bare remote: {err}"));
    assert!(status.success());
}

fn set_origin_remote(repo_path: &Path, remote_path: &Path) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "remote",
            "add",
            "origin",
            remote_path.to_string_lossy().as_ref(),
        ])
        .status()
        .unwrap_or_else(|err| panic!("failed to set origin remote: {err}"));
    assert!(status.success());
}

fn wait_for_remote_commits(remote_path: &Path, expected: i64, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() <= timeout {
        let count = remote_commit_count(remote_path);
        if count >= expected {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn remote_commit_count(remote_path: &Path) -> i64 {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(remote_path)
        .args(["rev-list", "--count", "--all"])
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
