use std::sync::mpsc;

use crate::error::{Error, Result};
use crate::executor::{GitCommand, oneshot};
use crate::state::SharedSystemState;

const RAW_BRANCH: &str = "raw";
const CHECKPOINT_BRANCH: &str = "checkpoint";

pub fn checkpoint(
    command_tx: &mpsc::Sender<GitCommand>,
    state: &SharedSystemState,
    timestamp: &str,
) -> Result<()> {
    state.enter_exclusive("checkpoint")?;

    let checkpoint_result = run_checkpoint(command_tx, timestamp);
    let cleanup_result = send_checkout(command_tx, RAW_BRANCH);
    let checkpoint_result = match (checkpoint_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(checkpoint_err), Err(cleanup_err)) => Err(Error::Git(format!(
            "checkpoint failed: {checkpoint_err}; failed to restore raw branch: {cleanup_err}"
        ))),
    };
    let exit_result = state.exit_exclusive();
    match (checkpoint_result, exit_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(err)) => Err(err),
        (Err(checkpoint_err), Err(exit_err)) => Err(Error::Watcher(format!(
            "checkpoint failed: {checkpoint_err}; failed to exit exclusive state: {exit_err}"
        ))),
    }
}

fn run_checkpoint(command_tx: &mpsc::Sender<GitCommand>, timestamp: &str) -> Result<()> {
    send_barrier(command_tx)?;
    ensure_raw_branch(command_tx)?;
    ensure_checkpoint_branch(command_tx)?;
    send_checkout(command_tx, CHECKPOINT_BRANCH)?;
    send_merge_squash(command_tx, RAW_BRANCH)?;
    send_commit(command_tx, &format!("checkpoint: {timestamp}"))?;
    Ok(())
}

fn ensure_raw_branch(command_tx: &mpsc::Sender<GitCommand>) -> Result<()> {
    match send_checkout(command_tx, RAW_BRANCH) {
        Ok(()) => Ok(()),
        Err(err) if is_unknown_start_point_error(&err) => {
            send_create_branch(command_tx, RAW_BRANCH, None)?;
            send_checkout(command_tx, RAW_BRANCH)
        }
        Err(err) => Err(err),
    }
}

fn ensure_checkpoint_branch(command_tx: &mpsc::Sender<GitCommand>) -> Result<()> {
    for start_point in [Some("main"), Some("master"), Some(RAW_BRANCH), None] {
        match send_create_branch(command_tx, CHECKPOINT_BRANCH, start_point) {
            Ok(()) => return Ok(()),
            Err(err) if is_branch_exists_error(&err) => return Ok(()),
            Err(err) if is_unknown_start_point_error(&err) => continue,
            Err(err) => return Err(err),
        }
    }

    Err(Error::Git(
        "failed to ensure checkpoint branch from available start points".to_owned(),
    ))
}

fn send_barrier(command_tx: &mpsc::Sender<GitCommand>) -> Result<()> {
    let (barrier_tx, barrier_rx) = oneshot();
    command_tx
        .send(GitCommand::Barrier { reply: barrier_tx })
        .map_err(|source| Error::Watcher(format!("failed to send barrier command: {source}")))?;
    barrier_rx.recv().map_err(|source| {
        Error::Watcher(format!(
            "failed to receive barrier acknowledgement for checkpoint: {source}"
        ))
    })?;
    Ok(())
}

fn send_checkout(command_tx: &mpsc::Sender<GitCommand>, branch: &str) -> Result<()> {
    let (reply_tx, reply_rx) = oneshot();
    command_tx
        .send(GitCommand::CheckoutBranch {
            name: branch.to_owned(),
            reply: reply_tx,
        })
        .map_err(|source| Error::Git(format!("failed to send checkout command: {source}")))?;
    reply_rx
        .recv()
        .map_err(|source| Error::Git(format!("failed to receive checkout result: {source}")))?
}

fn send_create_branch(
    command_tx: &mpsc::Sender<GitCommand>,
    branch: &str,
    start_point: Option<&str>,
) -> Result<()> {
    let (reply_tx, reply_rx) = oneshot();
    command_tx
        .send(GitCommand::CreateBranch {
            name: branch.to_owned(),
            start_point: start_point.map(str::to_owned),
            reply: reply_tx,
        })
        .map_err(|source| Error::Git(format!("failed to send create-branch command: {source}")))?;
    reply_rx.recv().map_err(|source| {
        Error::Git(format!(
            "failed to receive create-branch command result: {source}"
        ))
    })?
}

fn send_merge_squash(command_tx: &mpsc::Sender<GitCommand>, from: &str) -> Result<()> {
    let (reply_tx, reply_rx) = oneshot();
    command_tx
        .send(GitCommand::MergeSquash {
            from: from.to_owned(),
            reply: reply_tx,
        })
        .map_err(|source| Error::Git(format!("failed to send merge-squash command: {source}")))?;
    reply_rx.recv().map_err(|source| {
        Error::Git(format!(
            "failed to receive merge-squash command result: {source}"
        ))
    })?
}

fn send_commit(command_tx: &mpsc::Sender<GitCommand>, message: &str) -> Result<()> {
    let (reply_tx, reply_rx) = oneshot();
    command_tx
        .send(GitCommand::Commit {
            message: message.to_owned(),
            reply: reply_tx,
        })
        .map_err(|source| {
            Error::Git(format!(
                "failed to send checkpoint commit command: {source}"
            ))
        })?;
    let result = reply_rx.recv().map_err(|source| {
        Error::Git(format!(
            "failed to receive checkpoint commit result from executor: {source}"
        ))
    })?;
    let _hash = result?;
    Ok(())
}

fn is_branch_exists_error(err: &Error) -> bool {
    err.to_string().contains("already exists")
}

fn is_unknown_start_point_error(err: &Error) -> bool {
    let message = err.to_string();
    message.contains("not a valid object name")
        || message.contains("unknown revision")
        || message.contains("not found")
        || message.contains("did not match any file(s) known to git")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::checkpoint;
    use crate::executor::GitExecutor;
    use crate::git::GitRepo;
    use crate::state::SharedSystemState;

    #[test]
    fn test_checkpoint_creates_squash_commit_on_checkpoint_branch() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));

        fs::write(dir.path().join("SOUL.md"), "seed\n")
            .unwrap_or_else(|err| panic!("failed to write seed file: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add seed failed: {err}"));
        repo.commit("seed")
            .unwrap_or_else(|err| panic!("seed commit failed: {err}"));
        repo.run_git_args(["checkout", "-b", "raw"])
            .unwrap_or_else(|err| panic!("create raw branch failed: {err}"));

        fs::write(dir.path().join("SOUL.md"), "raw-v1\n")
            .unwrap_or_else(|err| panic!("failed to write raw v1: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add raw v1 failed: {err}"));
        repo.commit("raw v1")
            .unwrap_or_else(|err| panic!("commit raw v1 failed: {err}"));
        fs::write(dir.path().join("SOUL.md"), "raw-v2\n")
            .unwrap_or_else(|err| panic!("failed to write raw v2: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add raw v2 failed: {err}"));
        repo.commit("raw v2")
            .unwrap_or_else(|err| panic!("commit raw v2 failed: {err}"));

        let mut executor = GitExecutor::start(repo.clone());
        let state = SharedSystemState::new_running();
        let result = checkpoint(&executor.sender(), &state, "2026-02-18T00:00:00Z");
        assert!(result.is_ok());

        let current_branch = repo
            .current_branch()
            .unwrap_or_else(|err| panic!("get current branch failed: {err}"));
        assert_eq!(current_branch, "raw");

        let checkpoint_message = repo
            .run_git_args(["log", "-1", "--pretty=%s", "checkpoint"])
            .unwrap_or_else(|err| panic!("read checkpoint log failed: {err}"));
        assert!(checkpoint_message.starts_with("checkpoint: "));

        let raw_tree = repo
            .run_git_args(["rev-parse", "raw^{tree}"])
            .unwrap_or_else(|err| panic!("read raw tree hash failed: {err}"));
        let checkpoint_tree = repo
            .run_git_args(["rev-parse", "checkpoint^{tree}"])
            .unwrap_or_else(|err| panic!("read checkpoint tree hash failed: {err}"));
        assert_eq!(raw_tree, checkpoint_tree);
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_checkpoint_creates_raw_branch_if_missing() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        fs::write(dir.path().join("SOUL.md"), "seed\n")
            .unwrap_or_else(|err| panic!("failed to write seed file: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add seed failed: {err}"));
        repo.commit("seed")
            .unwrap_or_else(|err| panic!("seed commit failed: {err}"));

        let mut executor = GitExecutor::start(repo.clone());
        let state = SharedSystemState::new_running();
        let _ = checkpoint(&executor.sender(), &state, "2026-02-18T00:00:00Z");

        let raw_exists = repo.run_git_args(["rev-parse", "--verify", "raw"]);
        assert!(raw_exists.is_ok());
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_checkpoint_failure_restores_raw_branch() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));

        fs::write(dir.path().join("SOUL.md"), "seed\n")
            .unwrap_or_else(|err| panic!("failed to write seed file: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add seed failed: {err}"));
        repo.commit("seed")
            .unwrap_or_else(|err| panic!("seed commit failed: {err}"));
        repo.run_git_args(["checkout", "-b", "raw"])
            .unwrap_or_else(|err| panic!("create raw branch failed: {err}"));
        fs::write(dir.path().join("SOUL.md"), "raw-v1\n")
            .unwrap_or_else(|err| panic!("failed to write raw v1 file: {err}"));
        repo.add(Path::new("SOUL.md"))
            .unwrap_or_else(|err| panic!("add raw v1 failed: {err}"));
        repo.commit("raw v1")
            .unwrap_or_else(|err| panic!("raw v1 commit failed: {err}"));

        let mut executor = GitExecutor::start(repo.clone());
        let state = SharedSystemState::new_running();
        let first = checkpoint(&executor.sender(), &state, "2026-02-18T00:00:00Z");
        assert!(first.is_ok());
        let second = checkpoint(&executor.sender(), &state, "2026-02-18T00:10:00Z");
        assert!(second.is_err());

        let current_branch = repo
            .current_branch()
            .unwrap_or_else(|err| panic!("get current branch failed: {err}"));
        assert_eq!(current_branch, "raw");
        assert!(executor.stop().is_ok());
    }
}
