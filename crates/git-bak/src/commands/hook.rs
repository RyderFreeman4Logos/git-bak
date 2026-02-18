use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use git_bak_core::{Error, GitExecutor, GitRepo, HookHandler, HookSignal, WorkspaceConfig};

pub fn execute(event: String) -> Result<(), Error> {
    let config_path = config_path()?;
    execute_with_config_path(event, config_path.as_path())
}

fn execute_with_config_path(event: String, config_path: &Path) -> Result<(), Error> {
    let mut config = WorkspaceConfig::load(config_path)?;
    config.workspace = normalize_workspace_path(config.workspace.as_path())?;
    let repo = GitRepo::new(&config.workspace)?;
    let mut executor = GitExecutor::start(repo);
    let handler = HookHandler::new(&config, executor.sender());
    handler.ensure_signal_dir()?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| Error::Hook(format!("failed to get system time for hook event: {source}")))?;
    let signal = HookSignal {
        event,
        dedupe_key: format!("manual-{}", now.as_nanos()),
        timestamp: now.as_secs().to_string(),
    };
    let committed = handler.process_signal(&signal)?;
    executor.stop()?;
    println!("hook event processed, commit attempts: {committed}");
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
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::execute_with_config_path;
    use git_bak_core::GitRepo;

    #[test]
    fn test_hook_cli_fires_single_commit() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        fs::write(dir.path().join("SOUL.md"), "hook cli\n")
            .unwrap_or_else(|err| panic!("write watched file failed: {err}"));
        let config = format!(
            "workspace = \"{}\"\nwatch = [\"SOUL.md\"]\ndebounce_ms = 100\npush_interval_sec = 600\nstorm_window_sec = 5\nhook_signal_dir = \".git-bak/hooks\"\nmode = \"hook\"\n",
            dir.path().to_string_lossy().replace('\\', "\\\\")
        );
        let config_path = dir.path().join("personaguard.toml");
        fs::write(&config_path, config).unwrap_or_else(|err| panic!("write config failed: {err}"));

        let result = execute_with_config_path("command:stop".to_owned(), config_path.as_path());
        assert!(result.is_ok());

        let commit_count = git_commit_count(repo.path());
        assert!(commit_count >= 1);
    }

    fn git_commit_count(repo_path: &Path) -> i64 {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["rev-list", "--count", "HEAD"])
            .output();
        let output = match output {
            Ok(output) => output,
            Err(_) => return 0,
        };
        if !output.status.success() {
            return 0;
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<i64>()
            .unwrap_or(0)
    }
}
