use std::path::PathBuf;
use std::process::Command;

use git_bak_core::{Error, GitRepo, ProcessLock, WorkspaceConfig};

pub fn execute() -> Result<(), Error> {
    let config_path = config_path()?;
    let config = WorkspaceConfig::load(&config_path)?;
    let repo = GitRepo::new(&config.workspace)?;

    println!("config: {}", config_path.display());
    println!("workspace: {}", config.workspace.display());
    println!("watch count: {}", config.watch.len());
    println!("debounce_ms: {}", config.debounce_ms);
    println!("push_interval_sec: {}", config.push_interval_sec);
    println!("mode: {:?}", config.mode);

    let porcelain = repo.status()?;
    if porcelain.is_empty() {
        println!("git status: clean");
    } else {
        println!("git status:\n{porcelain}");
    }

    let log = recent_commits(repo.path())?;
    println!("recent commits:\n{log}");

    let lock_status = match ProcessLock::acquire(repo.path()) {
        Ok(lock) => {
            let path = lock.path().to_path_buf();
            drop(lock);
            format!("unlocked ({})", path.display())
        }
        Err(_) => "locked".to_owned(),
    };
    println!("lock status: {lock_status}");

    Ok(())
}

fn config_path() -> Result<PathBuf, Error> {
    let current_dir = std::env::current_dir().map_err(|source| {
        Error::Config(format!("failed to resolve current directory: {source}"))
    })?;
    Ok(current_dir.join("personaguard.toml"))
}

fn recent_commits(repo_path: &std::path::Path) -> Result<String, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["log", "--oneline", "-5"])
        .output()
        .map_err(|source| {
            Error::Git(format!(
                "failed to read recent commits in {}: {source}",
                repo_path.display()
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::Git(format!(
            "failed to read recent commits in {}: {}",
            repo_path.display(),
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(stdout)
}
