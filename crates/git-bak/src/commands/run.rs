use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;

use git_bak_core::{Error, GitRepo, PersonaWatcher, ProcessLock, WorkspaceConfig};

pub async fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let config = WorkspaceConfig::load(&config_path)?;
    let repo = GitRepo::new(&config.workspace)?;

    recover_stale_lock(repo.path())?;
    let lock = ProcessLock::acquire(repo.path())?;

    let watcher = PersonaWatcher::new(&config, repo)?;
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());

    tokio::signal::ctrl_c()
        .await
        .map_err(|source| Error::Watcher(format!("failed to listen for Ctrl-C: {source}")))?;

    stop_handle.store(true, Ordering::Relaxed);
    match worker.join() {
        Ok(run_result) => run_result?,
        Err(_) => return Err(Error::Watcher("watcher thread panicked".to_owned())),
    }

    drop(lock);
    Ok(())
}

fn config_path() -> Result<PathBuf, Error> {
    let current_dir = std::env::current_dir()
        .map_err(|source| Error::Config(format!("failed to resolve current directory: {source}")))?;
    Ok(current_dir.join("personaguard.toml"))
}

fn recover_stale_lock(repo_path: &Path) -> Result<(), Error> {
    let lock_path = repo_path.join(".git").join("git-bak.lock");
    if !lock_path.exists() {
        return Ok(());
    }

    match ProcessLock::acquire(repo_path) {
        Ok(lock) => {
            drop(lock);
            fs::remove_file(&lock_path).map_err(|source| {
                Error::Lock(format!(
                    "failed to remove stale lock file {}: {source}",
                    lock_path.display()
                ))
            })?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}
