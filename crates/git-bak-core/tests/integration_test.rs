use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use git_bak_core::{
    GitExecutor, GitRepo, PersonaWatcher, SharedSystemState, WatchMode, WorkspaceConfig,
};
use tempfile::tempdir;

#[test]
fn test_init_and_commit() {
    let dir = match tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("failed to create tempdir: {err}"),
    };

    let repo = match GitRepo::init(dir.path()) {
        Ok(repo) => repo,
        Err(err) => panic!("failed to init repository: {err}"),
    };
    setup_git_identity(repo.path());

    let file_path = dir.path().join("SOUL.md");
    let write_result = fs::write(&file_path, "persona\n");
    assert!(write_result.is_ok());

    let add_result = repo.add(Path::new("SOUL.md"));
    assert!(add_result.is_ok());

    let commit_result = repo.commit("test commit");
    assert!(commit_result.is_ok());

    let status = match repo.status() {
        Ok(status) => status,
        Err(err) => panic!("failed to get status: {err}"),
    };
    assert!(status.is_empty());
}

#[test]
fn test_watcher_pipeline() {
    let dir = match tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("failed to create tempdir: {err}"),
    };
    let repo = match GitRepo::init(dir.path()) {
        Ok(repo) => repo,
        Err(err) => panic!("failed to init repository: {err}"),
    };
    setup_git_identity(repo.path());

    let config = WorkspaceConfig {
        workspace: dir.path().to_path_buf(),
        watch: vec!["SOUL.md".to_owned()],
        debounce_ms: 100,
        push_interval_sec: 600,
        storm_window_sec: 5,
        hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
        mode: WatchMode::Watcher,
    };
    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();

    let watcher = match PersonaWatcher::new(&config, executor.sender(), state) {
        Ok(watcher) => watcher,
        Err(err) => panic!("failed to create watcher: {err}"),
    };
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());

    thread::sleep(Duration::from_millis(250));
    let write_result = fs::write(dir.path().join("SOUL.md"), "updated persona\n");
    assert!(write_result.is_ok());

    let mut commit_count = 0;
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(100));
        commit_count = git_commit_count(repo.path());
        if commit_count > 0 {
            break;
        }
    }

    stop_handle.store(true, Ordering::Relaxed);
    let join_result = worker.join();
    assert!(join_result.is_ok());
    let run_result = match join_result {
        Ok(result) => result,
        Err(_) => panic!("watcher thread panicked"),
    };
    assert!(run_result.is_ok());
    assert!(commit_count > 0);
    assert!(executor.stop().is_ok());
}

#[test]
fn test_watcher_pipeline_tracks_deletion() {
    let dir = match tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("failed to create tempdir: {err}"),
    };
    let repo = match GitRepo::init(dir.path()) {
        Ok(repo) => repo,
        Err(err) => panic!("failed to init repository: {err}"),
    };
    setup_git_identity(repo.path());

    let file_path = dir.path().join("SOUL.md");
    let seed_write = fs::write(&file_path, "seed\n");
    assert!(seed_write.is_ok());
    let seed_add = repo.add(Path::new("SOUL.md"));
    assert!(seed_add.is_ok());
    let seed_commit = repo.commit("seed");
    assert!(seed_commit.is_ok());

    let config = WorkspaceConfig {
        workspace: dir.path().to_path_buf(),
        watch: vec!["SOUL.md".to_owned()],
        debounce_ms: 100,
        push_interval_sec: 600,
        storm_window_sec: 5,
        hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
        mode: WatchMode::Watcher,
    };
    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();

    let watcher = match PersonaWatcher::new(&config, executor.sender(), state) {
        Ok(watcher) => watcher,
        Err(err) => panic!("failed to create watcher: {err}"),
    };
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());

    thread::sleep(Duration::from_millis(250));
    let remove_result = fs::remove_file(&file_path);
    assert!(remove_result.is_ok());

    let mut commit_count = 0;
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        commit_count = git_commit_count(repo.path());
        if commit_count >= 2 {
            break;
        }
    }

    stop_handle.store(true, Ordering::Relaxed);
    let join_result = worker.join();
    assert!(join_result.is_ok());
    let run_result = match join_result {
        Ok(result) => result,
        Err(_) => panic!("watcher thread panicked"),
    };
    assert!(run_result.is_ok());
    assert!(commit_count >= 2);
    assert!(executor.stop().is_ok());
}

fn setup_git_identity(repo_path: &Path) {
    let name_status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["config", "user.name", "git-bak-test"])
        .status();
    match name_status {
        Ok(status) => assert!(status.success()),
        Err(err) => panic!("failed to configure user.name: {err}"),
    }

    let email_status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["config", "user.email", "git-bak@example.com"])
        .status();
    match email_status {
        Ok(status) => assert!(status.success()),
        Err(err) => panic!("failed to configure user.email: {err}"),
    }
}

fn git_commit_count(repo_path: &Path) -> i64 {
    let output = std::process::Command::new("git")
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

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    raw.parse::<i64>().unwrap_or(0)
}
