use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use git_bak_core::{
    Error, GitExecutor, GitRepo, ProcessLock, SharedSystemState, WorkspaceConfig, checkpoint,
};

pub fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let mut config = WorkspaceConfig::load(&config_path)?;
    config.workspace = normalize_workspace_path(config.workspace.as_path())?;
    let repo = GitRepo::new(&config.workspace)?;
    let _lock = ProcessLock::acquire(repo.path())?;

    let mut executor = GitExecutor::start(repo);
    let state = SharedSystemState::new_running();
    let timestamp = current_timestamp_string()?;
    checkpoint(&executor.sender(), &state, &timestamp)?;
    executor.stop()?;
    println!("checkpoint created at {}", timestamp);
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

fn current_timestamp_string() -> Result<String, Error> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| Error::Config(format!("failed to get system time: {source}")))?;
    Ok(format!("{}", duration.as_secs()))
}
