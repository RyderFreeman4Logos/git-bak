use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;

use git_bak_core::{
    Error, GitExecutor, GitRepo, PersonaWatcher, ProcessLock, SharedSystemState, WatchMode,
    WorkspaceConfig,
};

pub async fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let mut config = WorkspaceConfig::load(&config_path)?;
    config.workspace = normalize_workspace_path(config.workspace.as_path())?;
    let repo = GitRepo::new(&config.workspace)?;

    match config.mode {
        WatchMode::Watcher | WatchMode::Hook => run_watcher_mode(&config, repo).await,
    }
}

async fn run_watcher_mode(config: &WorkspaceConfig, repo: GitRepo) -> Result<(), Error> {
    let _lock = ProcessLock::acquire(repo.path())?;
    let mut executor = GitExecutor::start(repo);
    let state = SharedSystemState::new_running();

    let watcher = PersonaWatcher::new(config, executor.sender(), state)?;
    let stop_handle = watcher.stop_handle();
    let worker = thread::spawn(move || watcher.run());
    let mut join_task = tokio::task::spawn_blocking(move || worker.join());

    enum ExitReason {
        CtrlC,
        Worker(Result<(), Error>),
    }

    let exit_reason = tokio::select! {
        ctrl_c_result = tokio::signal::ctrl_c() => {
            ctrl_c_result.map_err(|source| Error::Watcher(format!("failed to listen for Ctrl-C: {source}")))?;
            ExitReason::CtrlC
        }
        worker_result = &mut join_task => {
            ExitReason::Worker(unwrap_join_task_result(worker_result)?)
        }
    };

    let result = match exit_reason {
        ExitReason::CtrlC => {
            stop_handle.store(true, Ordering::Relaxed);
            unwrap_join_task_result(join_task.await)?
        }
        ExitReason::Worker(run_result) => match run_result {
            Ok(()) => Err(Error::Watcher(
                "watcher stopped unexpectedly before Ctrl-C".to_owned(),
            )),
            Err(err) => Err(err),
        },
    };

    executor.stop()?;
    result
}

fn unwrap_join_task_result(
    join_task_result: std::result::Result<
        std::thread::Result<Result<(), Error>>,
        tokio::task::JoinError,
    >,
) -> Result<Result<(), Error>, Error> {
    let join_result = join_task_result
        .map_err(|source| Error::Watcher(format!("failed to join watcher task: {source}")))?;
    join_result.map_err(|_| Error::Watcher("watcher thread panicked".to_owned()))
}

fn config_path() -> Result<PathBuf, Error> {
    let current_dir = std::env::current_dir().map_err(|source| {
        Error::Config(format!("failed to resolve current directory: {source}"))
    })?;
    Ok(current_dir.join("personaguard.toml"))
}

fn normalize_workspace_path(path: &Path) -> Result<PathBuf, Error> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| {
                Error::Config(format!("failed to resolve current directory: {source}"))
            })?
            .join(path)
    };

    fs::canonicalize(&candidate).map_err(|source| {
        Error::Config(format!(
            "failed to normalize workspace path {}: {source}",
            candidate.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::normalize_workspace_path;

    #[test]
    fn test_normalize_workspace_path_resolves_relative_path() {
        let normalized = normalize_workspace_path(Path::new("."));
        assert!(normalized.is_ok());
        assert!(normalized.unwrap_or_default().is_absolute());
    }
}
