use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use git_bak_core::{
    Error, GitCommand, GitExecutor, GitRepo, HookHandler, PersonaWatcher, ProcessLock,
    PushScheduler, SharedSystemState, WatchMode, WorkspaceConfig, oneshot,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{info, warn};

pub async fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let mut config = WorkspaceConfig::load(&config_path)?;
    config.workspace = normalize_workspace_path(config.workspace.as_path())?;
    let repo = GitRepo::new(&config.workspace)?;

    match config.mode {
        WatchMode::Watcher => run_watcher_mode(&config, repo).await,
        WatchMode::Hook => run_hook_mode(&config, repo).await,
    }
}

async fn run_watcher_mode(config: &WorkspaceConfig, repo: GitRepo) -> Result<(), Error> {
    let _lock = ProcessLock::acquire(repo.path())?;
    let mut executor = GitExecutor::start(repo);
    let mut push_scheduler =
        PushScheduler::start(config, executor.sender(), executor.commit_count());
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

    push_scheduler.stop()?;
    drain_executor(&executor)?;
    executor.stop()?;
    result
}

async fn run_hook_mode(config: &WorkspaceConfig, repo: GitRepo) -> Result<(), Error> {
    let _lock = ProcessLock::acquire(repo.path())?;
    let mut executor = GitExecutor::start(repo);
    let mut push_scheduler =
        PushScheduler::start(config, executor.sender(), executor.commit_count());
    let handler = HookHandler::new(config, executor.sender());
    handler.ensure_signal_dir()?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let worker_flag = Arc::clone(&stop_flag);
    let worker = thread::spawn(move || run_hook_event_loop(handler, worker_flag));
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
            stop_flag.store(true, Ordering::Relaxed);
            unwrap_join_task_result(join_task.await)?
        }
        ExitReason::Worker(run_result) => match run_result {
            Ok(()) => Err(Error::Watcher(
                "hook watcher stopped unexpectedly before Ctrl-C".to_owned(),
            )),
            Err(err) => Err(err),
        },
    };

    push_scheduler.stop()?;
    drain_executor(&executor)?;
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

fn run_hook_event_loop(handler: HookHandler, stop_flag: Arc<AtomicBool>) -> Result<(), Error> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let signal_dir = handler.signal_dir().to_path_buf();

    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })
    .map_err(|source| {
        Error::Hook(format!(
            "failed to create hook signal watcher for {}: {source}",
            signal_dir.display()
        ))
    })?;
    watcher
        .watch(signal_dir.as_path(), RecursiveMode::NonRecursive)
        .map_err(|source| {
            Error::Hook(format!(
                "failed to watch hook signal directory {}: {source}",
                signal_dir.display()
            ))
        })?;

    while !stop_flag.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(event)) => {
                for path in event.paths {
                    if let Err(err) = handler.handle_signal_file(path.as_path()) {
                        warn!("failed to process hook signal {}: {}", path.display(), err);
                    }
                }
            }
            Ok(Err(err)) => warn!("hook signal watcher error: {err}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Hook(
                    "hook signal watcher channel disconnected unexpectedly".to_owned(),
                ));
            }
        }
    }

    info!("hook signal watcher stopped");
    Ok(())
}

fn drain_executor(executor: &GitExecutor) -> Result<(), Error> {
    let (barrier_tx, barrier_rx) = oneshot();
    executor
        .sender()
        .send(GitCommand::Barrier { reply: barrier_tx })
        .map_err(|source| Error::Watcher(format!("failed to send barrier command: {source}")))?;
    barrier_rx.recv().map_err(|source| {
        Error::Watcher(format!(
            "failed to receive barrier acknowledgement before shutdown: {source}"
        ))
    })?;
    Ok(())
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
