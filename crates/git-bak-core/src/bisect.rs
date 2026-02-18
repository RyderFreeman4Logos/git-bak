use std::ffi::OsStr;
use std::sync::mpsc;

use crate::error::{Error, Result};
use crate::executor::{GitCommand, oneshot};
use crate::git::GitRepo;
use crate::state::SharedSystemState;

pub fn bisect_run(
    command_tx: &mpsc::Sender<GitCommand>,
    state: &SharedSystemState,
    repo: &GitRepo,
    good_ref: &str,
    bad_ref: &str,
    healthcheck_cmd: &str,
) -> Result<String> {
    state.enter_exclusive("bisect")?;

    let operation_result = run_bisect(command_tx, repo, good_ref, bad_ref, healthcheck_cmd);
    let reset_result = repo.run_git_args(["bisect", "reset"]);
    let exit_result = state.exit_exclusive();

    match operation_result {
        Ok(output) => {
            if let Err(err) = reset_result {
                return Err(Error::Git(format!(
                    "bisect completed but reset failed: {err}"
                )));
            }
            if let Err(err) = exit_result {
                return Err(err);
            }
            Ok(output)
        }
        Err(err) => {
            let reset_suffix = match reset_result {
                Ok(_) => String::new(),
                Err(reset_err) => format!("; failed to reset bisect state: {reset_err}"),
            };
            let exit_suffix = match exit_result {
                Ok(_) => String::new(),
                Err(exit_err) => format!("; failed to exit exclusive state: {exit_err}"),
            };
            Err(Error::Git(format!(
                "bisect run failed: {err}{reset_suffix}{exit_suffix}"
            )))
        }
    }
}

fn run_bisect(
    command_tx: &mpsc::Sender<GitCommand>,
    repo: &GitRepo,
    good_ref: &str,
    bad_ref: &str,
    healthcheck_cmd: &str,
) -> Result<String> {
    send_barrier(command_tx)?;
    repo.run_git_args(["bisect", "start"])?;
    repo.run_git_args(["bisect", "good", good_ref])?;
    repo.run_git_args(["bisect", "bad", bad_ref])?;
    repo.run_git_os_args([
        OsStr::new("bisect"),
        OsStr::new("run"),
        OsStr::new("sh"),
        OsStr::new("-c"),
        OsStr::new(healthcheck_cmd),
    ])
}

fn send_barrier(command_tx: &mpsc::Sender<GitCommand>) -> Result<()> {
    let (barrier_tx, barrier_rx) = oneshot();
    command_tx
        .send(GitCommand::Barrier { reply: barrier_tx })
        .map_err(|source| Error::Watcher(format!("failed to send bisect barrier command: {source}")))?;
    barrier_rx.recv().map_err(|source| {
        Error::Watcher(format!(
            "failed to receive bisect barrier acknowledgement: {source}"
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::bisect_run;
    use crate::executor::GitExecutor;
    use crate::git::GitRepo;
    use crate::state::{SharedSystemState, SystemState};

    #[test]
    fn test_bisect_finds_first_bad_commit() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));

        fs::write(dir.path().join("marker.txt"), "good\n")
            .unwrap_or_else(|err| panic!("failed to write marker: {err}"));
        repo.add(Path::new("marker.txt"))
            .unwrap_or_else(|err| panic!("add marker failed: {err}"));
        repo.commit("good seed")
            .unwrap_or_else(|err| panic!("seed commit failed: {err}"));

        fs::write(dir.path().join("marker.txt"), "good version\n")
            .unwrap_or_else(|err| panic!("failed to write good marker: {err}"));
        repo.add(Path::new("marker.txt"))
            .unwrap_or_else(|err| panic!("add good marker failed: {err}"));
        let good_hash = repo
            .commit("good commit")
            .unwrap_or_else(|err| panic!("good commit failed: {err}"));

        fs::write(dir.path().join("marker.txt"), "bad version 1\n")
            .unwrap_or_else(|err| panic!("failed to write bad marker 1: {err}"));
        repo.add(Path::new("marker.txt"))
            .unwrap_or_else(|err| panic!("add bad marker 1 failed: {err}"));
        let first_bad_hash = repo
            .commit("bad commit one")
            .unwrap_or_else(|err| panic!("bad commit one failed: {err}"));

        fs::write(dir.path().join("marker.txt"), "bad version 2\n")
            .unwrap_or_else(|err| panic!("failed to write bad marker 2: {err}"));
        repo.add(Path::new("marker.txt"))
            .unwrap_or_else(|err| panic!("add bad marker 2 failed: {err}"));
        let bad_hash = repo
            .commit("bad commit two")
            .unwrap_or_else(|err| panic!("bad commit two failed: {err}"));

        let healthcheck_path = dir.path().join("healthcheck.sh");
        fs::write(
            &healthcheck_path,
            "#!/usr/bin/env bash\nif grep -q \"bad\" marker.txt; then exit 1; fi\nexit 0\n",
        )
        .unwrap_or_else(|err| panic!("failed to write healthcheck script: {err}"));
        let chmod_status = Command::new("chmod")
            .args(["+x", healthcheck_path.to_string_lossy().as_ref()])
            .status()
            .unwrap_or_else(|err| panic!("failed to chmod healthcheck script: {err}"));
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
        assert_eq!(
            state.current().unwrap_or(SystemState::Paused),
            SystemState::Running
        );
        assert!(executor.stop().is_ok());
    }
}
