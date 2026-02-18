use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::WorkspaceConfig;
use crate::error::{Error, Result};
use crate::executor::{GitCommand, oneshot};
use crate::filter::PathFilter;

const ALLOWED_EVENTS: [&str; 3] = ["command:new", "command:stop", "command:reset"];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HookSignal {
    pub event: String,
    pub dedupe_key: String,
    pub timestamp: String,
}

pub struct HookHandler {
    workspace: PathBuf,
    watch_patterns: Vec<String>,
    signal_dir: PathBuf,
    command_tx: mpsc::Sender<GitCommand>,
    seen_dedupe_keys: Mutex<HashSet<String>>,
}

impl HookHandler {
    pub fn new(config: &WorkspaceConfig, command_tx: mpsc::Sender<GitCommand>) -> Self {
        let signal_dir = if config.hook_signal_dir.is_absolute() {
            config.hook_signal_dir.clone()
        } else {
            config.workspace.join(&config.hook_signal_dir)
        };

        Self {
            workspace: config.workspace.clone(),
            watch_patterns: config.watch.clone(),
            signal_dir,
            command_tx,
            seen_dedupe_keys: Mutex::new(HashSet::new()),
        }
    }

    pub fn signal_dir(&self) -> &Path {
        &self.signal_dir
    }

    pub fn ensure_signal_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.signal_dir).map_err(|source| {
            Error::Hook(format!(
                "failed to create hook signal directory {}: {source}",
                self.signal_dir.display()
            ))
        })
    }

    pub fn parse_signal_json(content: &str) -> Result<HookSignal> {
        let signal: HookSignal = serde_json::from_str(content)
            .map_err(|source| Error::Hook(format!("invalid hook signal json: {source}")))?;
        validate_signal(&signal)?;
        Ok(signal)
    }

    pub fn handle_signal_file(&self, signal_path: &Path) -> Result<bool> {
        let file_name = signal_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if file_name.ends_with(".tmp") {
            debug!("ignore temporary hook signal file {}", signal_path.display());
            return Ok(false);
        }
        if !signal_path.is_file() {
            return Ok(false);
        }

        let content = fs::read_to_string(signal_path).map_err(|source| {
            Error::Hook(format!(
                "failed to read hook signal {}: {source}",
                signal_path.display()
            ))
        })?;
        let signal = Self::parse_signal_json(&content)?;
        if self.is_duplicate_dedupe_key(&signal.dedupe_key)? {
            info!(
                "skip duplicate hook signal key {} from {}",
                signal.dedupe_key,
                signal_path.display()
            );
            return Ok(false);
        }

        self.process_signal(&signal)?;
        Ok(true)
    }

    pub fn process_signal(&self, signal: &HookSignal) -> Result<usize> {
        validate_signal(signal)?;

        let watched_paths = collect_watched_files(&self.workspace, &self.watch_patterns)?;
        let mut committed = 0usize;
        for watched_path in watched_paths {
            let relative_path = match watched_path.strip_prefix(&self.workspace) {
                Ok(path) => path.to_path_buf(),
                Err(_) => watched_path.clone(),
            };
            if let Err(err) = self.commit_path_if_changed(signal, &relative_path) {
                warn!(
                    "failed to commit watched path {} for signal {}: {}",
                    relative_path.display(),
                    signal.dedupe_key,
                    err
                );
            } else {
                committed += 1;
            }
        }

        info!(
            "processed hook signal {} ({}) with {} commit attempts",
            signal.dedupe_key, signal.event, committed
        );
        Ok(committed)
    }

    fn commit_path_if_changed(&self, signal: &HookSignal, relative_path: &Path) -> Result<()> {
        let (add_tx, add_rx) = oneshot();
        self.command_tx
            .send(GitCommand::Add {
                path: relative_path.to_path_buf(),
                reply: add_tx,
            })
            .map_err(|source| Error::Hook(format!("failed to enqueue add command: {source}")))?;
        let add_result = add_rx.recv().map_err(|source| {
            Error::Hook(format!("failed to receive add command result: {source}"))
        })?;
        add_result?;

        let (status_tx, status_rx) = oneshot();
        self.command_tx
            .send(GitCommand::Status { reply: status_tx })
            .map_err(|source| Error::Hook(format!("failed to enqueue status command: {source}")))?;
        let status_result = status_rx.recv().map_err(|source| {
            Error::Hook(format!("failed to receive status command result: {source}"))
        })?;
        let status = status_result?;
        let relative_path_text = relative_path.to_string_lossy();
        let has_path_change = status.lines().any(|line| line.ends_with(relative_path_text.as_ref()));
        if !has_path_change {
            return Ok(());
        }

        let (commit_tx, commit_rx) = oneshot();
        self.command_tx
            .send(GitCommand::CommitPath {
                path: relative_path.to_path_buf(),
                message: format!("chore: hook {} {}", signal.event, relative_path.display()),
                reply: commit_tx,
            })
            .map_err(|source| Error::Hook(format!("failed to enqueue commit command: {source}")))?;
        let commit_result = commit_rx.recv().map_err(|source| {
            Error::Hook(format!("failed to receive commit command result: {source}"))
        })?;
        commit_result?;
        Ok(())
    }

    fn is_duplicate_dedupe_key(&self, dedupe_key: &str) -> Result<bool> {
        let mut guard = self
            .seen_dedupe_keys
            .lock()
            .map_err(|_| Error::Hook("failed to lock hook dedupe key set".to_owned()))?;
        if guard.contains(dedupe_key) {
            return Ok(true);
        }
        guard.insert(dedupe_key.to_owned());
        Ok(false)
    }
}

fn validate_signal(signal: &HookSignal) -> Result<()> {
    if !ALLOWED_EVENTS.contains(&signal.event.as_str()) {
        return Err(Error::Hook(format!(
            "unsupported hook event {}, expected one of {:?}",
            signal.event, ALLOWED_EVENTS
        )));
    }
    if signal.dedupe_key.trim().is_empty() {
        return Err(Error::Hook("missing hook dedupe_key".to_owned()));
    }
    if signal.timestamp.trim().is_empty() {
        return Err(Error::Hook("missing hook timestamp".to_owned()));
    }
    Ok(())
}

fn collect_watched_files(workspace: &Path, watch_patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_recursive(workspace, &mut files)?;
    Ok(files
        .into_iter()
        .filter(|file_path| {
            let relative_path = file_path
                .strip_prefix(workspace)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| file_path.to_path_buf());
            !PathFilter::matches_denylist(&relative_path)
                && PathFilter::matches_allowlist(&relative_path, watch_patterns)
        })
        .collect())
}

fn collect_files_recursive(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(root).map_err(|source| {
        Error::Hook(format!(
            "failed to read directory while collecting watched files {}: {source}",
            root.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            Error::Hook(format!(
                "failed to read directory entry in {}: {source}",
                root.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            Error::Hook(format!(
                "failed to get file type while collecting {}: {source}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            collect_files_recursive(path.as_path(), out)?;
            continue;
        }
        if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{HookHandler, HookSignal};
    use crate::config::WorkspaceConfig;
    use crate::executor::GitExecutor;
    use crate::git::GitRepo;
    use crate::types::WatchMode;

    #[test]
    fn test_parse_valid_and_invalid_signal_files() {
        let valid = r#"{"event":"command:stop","dedupe_key":"01TEST","timestamp":"2026-01-01T00:00:00Z"}"#;
        let parsed = HookHandler::parse_signal_json(valid);
        assert!(parsed.is_ok());

        let invalid = r#"{"event":"unknown","dedupe_key":"01TEST","timestamp":"2026-01-01T00:00:00Z"}"#;
        let parsed_invalid = HookHandler::parse_signal_json(invalid);
        assert!(parsed_invalid.is_err());
    }

    #[test]
    fn test_dedup_ignores_same_signal_key() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo);
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Hook,
        };
        let handler = HookHandler::new(&config, executor.sender());
        assert!(handler.ensure_signal_dir().is_ok());

        let signal_path = handler.signal_dir().join("signal-1.json");
        fs::write(
            &signal_path,
            r#"{"event":"command:new","dedupe_key":"01DUP","timestamp":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap_or_else(|err| panic!("write signal failed: {err}"));

        fs::write(dir.path().join("SOUL.md"), "hello\n")
            .unwrap_or_else(|err| panic!("write watched file failed: {err}"));
        let first = handler.handle_signal_file(signal_path.as_path());
        assert!(first.is_ok());
        assert!(first.unwrap_or(false));

        fs::write(dir.path().join("SOUL.md"), "hello v2\n")
            .unwrap_or_else(|err| panic!("rewrite watched file failed: {err}"));
        let second = handler.handle_signal_file(signal_path.as_path());
        assert!(second.is_ok());
        assert!(!second.unwrap_or(true));

        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_process_signal_commits_changed_watch_file() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo.clone());
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Hook,
        };
        let handler = HookHandler::new(&config, executor.sender());
        fs::write(dir.path().join("SOUL.md"), "hook content\n")
            .unwrap_or_else(|err| panic!("write watched file failed: {err}"));

        let signal = HookSignal {
            event: "command:stop".to_owned(),
            dedupe_key: "01HOOK".to_owned(),
            timestamp: "2026-01-01T00:00:00Z".to_owned(),
        };
        let committed = handler
            .process_signal(&signal)
            .unwrap_or_else(|err| panic!("process signal failed: {err}"));
        assert!(committed >= 1);
        assert!(executor.stop().is_ok());
    }
}
