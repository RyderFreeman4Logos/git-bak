use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::thread;

use git_bak_core::{Error, GitRepo, PersonaWatcher, ProcessLock, WatchMode, WorkspaceConfig};

pub async fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let config = WorkspaceConfig::load(&config_path)?;
    let repo = GitRepo::new(&config.workspace)?;

    match config.mode {
        WatchMode::Watcher => run_watcher_mode(&config, repo).await,
        WatchMode::Hook => run_hook_mode(),
    }
}

async fn run_watcher_mode(config: &WorkspaceConfig, repo: GitRepo) -> Result<(), Error> {
    let _lock = ProcessLock::acquire(repo.path())?;

    let watcher = PersonaWatcher::new(config, repo)?;
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

    Ok(())
}

fn run_hook_mode() -> Result<(), Error> {
    Err(Error::Hook(
        "hook mode is not managed by `git-bak run`; install and trigger the hook instead"
            .to_owned(),
    ))
}

fn config_path() -> Result<PathBuf, Error> {
    let current_dir = std::env::current_dir().map_err(|source| {
        Error::Config(format!("failed to resolve current directory: {source}"))
    })?;
    Ok(current_dir.join("personaguard.toml"))
}

#[cfg(test)]
mod tests {
    use git_bak_core::Error;

    use super::run_hook_mode;

    #[test]
    fn test_run_hook_mode_returns_clear_error() {
        let result = run_hook_mode();
        assert!(result.is_err());
        let err = result.err().unwrap_or_else(|| Error::Hook(String::new()));
        match err {
            Error::Hook(message) => assert!(message.contains("hook mode")),
            other => panic!("expected hook error, got {other}"),
        }
    }
}
