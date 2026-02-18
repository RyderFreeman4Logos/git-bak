use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use crate::error::{Error, Result};
use crate::git::GitRepo;

pub type OneShotSender<T> = mpsc::Sender<T>;
pub type OneShotReceiver<T> = mpsc::Receiver<T>;

pub fn oneshot<T>() -> (OneShotSender<T>, OneShotReceiver<T>) {
    mpsc::channel()
}

#[derive(Debug)]
pub enum GitCommand {
    Add {
        path: PathBuf,
        reply: OneShotSender<Result<()>>,
    },
    Commit {
        message: String,
        reply: OneShotSender<Result<String>>,
    },
    CommitPath {
        path: PathBuf,
        message: String,
        reply: OneShotSender<Result<String>>,
    },
    Push {
        remote: String,
        reply: OneShotSender<Result<()>>,
    },
    Tag {
        name: String,
        reply: OneShotSender<Result<()>>,
    },
    CheckoutBranch {
        name: String,
        reply: OneShotSender<Result<()>>,
    },
    MergeSquash {
        from: String,
        reply: OneShotSender<Result<()>>,
    },
    CreateBranch {
        name: String,
        start_point: Option<String>,
        reply: OneShotSender<Result<()>>,
    },
    Status {
        reply: OneShotSender<Result<String>>,
    },
    Barrier {
        reply: OneShotSender<()>,
    },
    Shutdown {
        reply: OneShotSender<()>,
    },
}

#[derive(Debug)]
pub struct GitExecutor {
    tx: mpsc::Sender<GitCommand>,
    worker: Option<JoinHandle<()>>,
    commit_count: Arc<AtomicUsize>,
}

impl GitExecutor {
    pub fn start(repo: GitRepo) -> Self {
        let (tx, rx) = mpsc::channel::<GitCommand>();
        let commit_count = Arc::new(AtomicUsize::new(0));
        let worker_commit_count = Arc::clone(&commit_count);
        let worker = thread::spawn(move || run_event_loop(repo, rx, worker_commit_count));

        Self {
            tx,
            worker: Some(worker),
            commit_count,
        }
    }

    pub fn sender(&self) -> mpsc::Sender<GitCommand> {
        self.tx.clone()
    }

    pub fn commit_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.commit_count)
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.worker.is_none() {
            return Ok(());
        }

        let (reply_tx, reply_rx) = oneshot();
        self.tx
            .send(GitCommand::Shutdown { reply: reply_tx })
            .map_err(|source| Error::Watcher(format!("failed to send shutdown command: {source}")))?;
        reply_rx.recv().map_err(|source| {
            Error::Watcher(format!(
                "failed to receive shutdown acknowledgment from executor: {source}"
            ))
        })?;

        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::Watcher("git executor thread panicked".to_owned()))?;
        }

        Ok(())
    }
}

impl Drop for GitExecutor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_event_loop(repo: GitRepo, rx: mpsc::Receiver<GitCommand>, commit_count: Arc<AtomicUsize>) {
    while let Ok(command) = rx.recv() {
        match command {
            GitCommand::Add { path, reply } => {
                let result = repo.add_all(path.as_path());
                let _ = reply.send(result);
            }
            GitCommand::Commit { message, reply } => {
                let result = repo.commit(&message);
                if result.is_ok() {
                    commit_count.fetch_add(1, Ordering::Relaxed);
                }
                let _ = reply.send(result);
            }
            GitCommand::CommitPath {
                path,
                message,
                reply,
            } => {
                let result = repo.commit_path(&message, path.as_path());
                if result.is_ok() {
                    commit_count.fetch_add(1, Ordering::Relaxed);
                }
                let _ = reply.send(result);
            }
            GitCommand::Push { remote, reply } => {
                let result = repo.push(&remote);
                let _ = reply.send(result);
            }
            GitCommand::Tag { name, reply } => {
                let result = repo.tag(&name);
                let _ = reply.send(result);
            }
            GitCommand::CheckoutBranch { name, reply } => {
                let result = repo.checkout_branch(&name);
                let _ = reply.send(result);
            }
            GitCommand::MergeSquash { from, reply } => {
                let result = repo.merge_squash(&from);
                let _ = reply.send(result);
            }
            GitCommand::CreateBranch {
                name,
                start_point,
                reply,
            } => {
                let result = repo.create_branch(&name, start_point.as_deref());
                let _ = reply.send(result);
            }
            GitCommand::Status { reply } => {
                let result = repo.status();
                let _ = reply.send(result);
            }
            GitCommand::Barrier { reply } => {
                let _ = reply.send(());
            }
            GitCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::Ordering;

    use tempfile::tempdir;

    use super::{GitCommand, GitExecutor, oneshot};
    use crate::git::GitRepo;

    #[test]
    fn test_executor_serializes_add_then_commit() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo.clone());
        let path = dir.path().join("SOUL.md");
        fs::write(&path, "v1\n").unwrap_or_else(|err| panic!("write failed: {err}"));

        let (add_tx, add_rx) = oneshot();
        executor
            .sender()
            .send(GitCommand::Add {
                path: Path::new("SOUL.md").to_path_buf(),
                reply: add_tx,
            })
            .unwrap_or_else(|err| panic!("failed to send add command: {err}"));
        let add_result = add_rx
            .recv()
            .unwrap_or_else(|err| panic!("failed to recv add reply: {err}"));
        assert!(add_result.is_ok());

        let (commit_tx, commit_rx) = oneshot();
        executor
            .sender()
            .send(GitCommand::Commit {
                message: "test add commit".to_owned(),
                reply: commit_tx,
            })
            .unwrap_or_else(|err| panic!("failed to send commit command: {err}"));
        let commit_result = commit_rx
            .recv()
            .unwrap_or_else(|err| panic!("failed to recv commit reply: {err}"));
        assert!(commit_result.is_ok());

        let status = repo.status().unwrap_or_else(|err| panic!("status failed: {err}"));
        assert!(status.is_empty());
        assert_eq!(executor.commit_count().load(Ordering::Relaxed), 1);
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_barrier_ack_arrives_after_prior_commands() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo.clone());
        fs::write(dir.path().join("SOUL.md"), "v1\n")
            .unwrap_or_else(|err| panic!("write failed: {err}"));

        let (add_tx, add_rx) = oneshot();
        executor
            .sender()
            .send(GitCommand::Add {
                path: Path::new("SOUL.md").to_path_buf(),
                reply: add_tx,
            })
            .unwrap_or_else(|err| panic!("failed to send add command: {err}"));
        let add_result = add_rx
            .recv()
            .unwrap_or_else(|err| panic!("failed to recv add reply: {err}"));
        assert!(add_result.is_ok());

        let (commit_tx, commit_rx) = oneshot();
        executor
            .sender()
            .send(GitCommand::Commit {
                message: "barrier test".to_owned(),
                reply: commit_tx,
            })
            .unwrap_or_else(|err| panic!("failed to send commit command: {err}"));

        let (barrier_tx, barrier_rx) = oneshot();
        executor
            .sender()
            .send(GitCommand::Barrier { reply: barrier_tx })
            .unwrap_or_else(|err| panic!("failed to send barrier command: {err}"));
        barrier_rx
            .recv()
            .unwrap_or_else(|err| panic!("failed to recv barrier ack: {err}"));

        let commit_result = commit_rx
            .recv()
            .unwrap_or_else(|err| panic!("failed to recv commit reply: {err}"));
        assert!(commit_result.is_ok());
        assert_eq!(executor.commit_count().load(Ordering::Relaxed), 1);
        assert!(executor.stop().is_ok());
    }
}
