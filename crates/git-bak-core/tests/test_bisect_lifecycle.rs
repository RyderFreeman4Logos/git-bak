use std::fs;
use std::path::Path;
use std::process::Command;

use git_bak_core::{GitExecutor, GitRepo, SharedSystemState, WatchMode, WorkspaceConfig, bisect_run};
use tempfile::tempdir;

#[test]
fn test_bisect_lifecycle() {
    let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
    let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
    let _config = WorkspaceConfig {
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

    fs::write(dir.path().join("marker.txt"), "good\n")
        .unwrap_or_else(|err| panic!("write seed marker failed: {err}"));
    repo.add(Path::new("marker.txt"))
        .unwrap_or_else(|err| panic!("add seed marker failed: {err}"));
    repo.commit("seed").unwrap_or_else(|err| panic!("seed commit failed: {err}"));

    fs::write(dir.path().join("marker.txt"), "good-v1\n")
        .unwrap_or_else(|err| panic!("write good marker failed: {err}"));
    repo.add(Path::new("marker.txt"))
        .unwrap_or_else(|err| panic!("add good marker failed: {err}"));
    let good_hash = repo
        .commit("good commit")
        .unwrap_or_else(|err| panic!("good commit failed: {err}"));

    fs::write(dir.path().join("marker.txt"), "bad-v1\n")
        .unwrap_or_else(|err| panic!("write bad marker 1 failed: {err}"));
    repo.add(Path::new("marker.txt"))
        .unwrap_or_else(|err| panic!("add bad marker 1 failed: {err}"));
    let first_bad_hash = repo
        .commit("bad commit 1")
        .unwrap_or_else(|err| panic!("bad commit 1 failed: {err}"));

    fs::write(dir.path().join("marker.txt"), "bad-v2\n")
        .unwrap_or_else(|err| panic!("write bad marker 2 failed: {err}"));
    repo.add(Path::new("marker.txt"))
        .unwrap_or_else(|err| panic!("add bad marker 2 failed: {err}"));
    let bad_hash = repo
        .commit("bad commit 2")
        .unwrap_or_else(|err| panic!("bad commit 2 failed: {err}"));

    let healthcheck_path = dir.path().join("healthcheck.sh");
    fs::write(
        &healthcheck_path,
        "#!/usr/bin/env bash\nif grep -q \"bad\" marker.txt; then exit 1; fi\nexit 0\n",
    )
    .unwrap_or_else(|err| panic!("write healthcheck failed: {err}"));
    let chmod_status = Command::new("chmod")
        .args(["+x", healthcheck_path.to_string_lossy().as_ref()])
        .status()
        .unwrap_or_else(|err| panic!("chmod failed: {err}"));
    assert!(chmod_status.success());

    let mut executor = GitExecutor::start(repo.clone());
    let state = SharedSystemState::new_running();
    let output = bisect_run(
        &executor.sender(),
        &state,
        &repo,
        &good_hash,
        &bad_hash,
        healthcheck_path.to_string_lossy().as_ref(),
    )
    .unwrap_or_else(|err| panic!("bisect run failed: {err}"));

    assert!(output.contains(first_bad_hash.as_str()));
    assert!(executor.stop().is_ok());
}
