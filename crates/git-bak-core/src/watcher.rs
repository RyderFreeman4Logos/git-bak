use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use tracing::{debug, info, warn};

use crate::config::WorkspaceConfig;
use crate::error::{Error, Result};
use crate::executor::{GitCommand, oneshot};
use crate::filter::PathFilter;
use crate::hash::is_stable;
use crate::state::SharedSystemState;

#[derive(Clone)]
pub struct PersonaWatcher {
    config: WorkspaceConfig,
    command_tx: mpsc::Sender<GitCommand>,
    state: SharedSystemState,
    stop_flag: Arc<AtomicBool>,
    last_seen: Arc<Mutex<HashMap<PathBuf, Instant>>>,
}

impl PersonaWatcher {
    pub fn new(
        config: &WorkspaceConfig,
        command_tx: mpsc::Sender<GitCommand>,
        state: SharedSystemState,
    ) -> Result<Self> {
        if !config.workspace.exists() {
            return Err(Error::Watcher(format!(
                "workspace path does not exist: {}",
                config.workspace.display()
            )));
        }

        Ok(Self {
            config: config.clone(),
            command_tx,
            state,
            stop_flag: Arc::new(AtomicBool::new(false)),
            last_seen: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn run(&self) -> Result<()> {
        let (tx, rx): (
            mpsc::Sender<DebounceEventResult>,
            mpsc::Receiver<DebounceEventResult>,
        ) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(self.config.debounce_ms), None, tx)
            .map_err(|source| Error::Watcher(format!("failed to create debouncer: {source}")))?;

        debouncer
            .watch(&self.config.workspace, RecursiveMode::Recursive)
            .map_err(|source| {
                Error::Watcher(format!(
                    "failed to watch workspace {}: {source}",
                    self.config.workspace.display()
                ))
            })?;

        while !self.stop_flag.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(result) => {
                    if let Err(err) = self.handle_debounce_result(result) {
                        warn!("watcher event handling failed: {err}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Watcher(
                        "watcher event channel disconnected unexpectedly".to_owned(),
                    ));
                }
            }
        }

        info!("persona watcher stopped");
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        self.state.pause()
    }

    pub fn resume(&self) -> Result<()> {
        self.state.resume()
    }

    pub fn drain(&self) -> Result<()> {
        self.state.start_draining()?;
        let (barrier_tx, barrier_rx) = oneshot();
        self.command_tx
            .send(GitCommand::Barrier { reply: barrier_tx })
            .map_err(|source| Error::Watcher(format!("failed to send barrier command: {source}")))?;
        barrier_rx.recv().map_err(|source| {
            Error::Watcher(format!("failed to receive barrier acknowledgement: {source}"))
        })?;
        Ok(())
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_flag)
    }

    fn handle_debounce_result(&self, result: DebounceEventResult) -> Result<()> {
        if !self.state.can_process_events() {
            return Ok(());
        }

        let events = match result {
            Ok(events) => events,
            Err(errors) => {
                let joined = errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<String>>()
                    .join("; ");
                warn!("watcher produced errors: {joined}");
                return Ok(());
            }
        };

        for event in events {
            for path in event.event.paths {
                if let Err(err) = self.handle_path(path) {
                    warn!("failed to process watched path: {err}");
                }
            }
        }

        Ok(())
    }

    fn handle_path(&self, absolute_path: PathBuf) -> Result<()> {
        if !self.state.can_process_events() {
            return Ok(());
        }

        let relative_path = relative_to_workspace(&self.config.workspace, &absolute_path);
        if PathFilter::matches_denylist(&relative_path) {
            debug!("skip denied path {}", relative_path.display());
            return Ok(());
        }

        if !PathFilter::matches_allowlist(&relative_path, &self.config.watch) {
            debug!("skip non-watched path {}", relative_path.display());
            return Ok(());
        }

        if self.should_suppress_storm(&relative_path)? {
            debug!(
                "skip rapid duplicate file event for {}",
                relative_path.display()
            );
            return Ok(());
        }

        let is_deleted = !absolute_path.exists();
        if !is_deleted && !absolute_path.is_file() {
            return Ok(());
        }

        if !is_deleted {
            let stable = is_stable(&absolute_path, self.config.debounce_ms)?;
            if !stable {
                warn!("skip unstable path {}", absolute_path.display());
                return Ok(());
            }
        }

        let (add_tx, add_rx) = oneshot();
        self.command_tx
            .send(GitCommand::Add {
                path: relative_path.clone(),
                reply: add_tx,
            })
            .map_err(|source| Error::Watcher(format!("failed to send add command: {source}")))?;
        let add_result = add_rx
            .recv()
            .map_err(|source| Error::Watcher(format!("failed to receive add result: {source}")))?;
        add_result?;

        let (status_tx, status_rx) = oneshot();
        self.command_tx
            .send(GitCommand::Status { reply: status_tx })
            .map_err(|source| Error::Watcher(format!("failed to send status command: {source}")))?;
        let status_result = status_rx.recv().map_err(|source| {
            Error::Watcher(format!("failed to receive status command result: {source}"))
        })?;
        let status = status_result?;
        if status.is_empty() {
            return Ok(());
        }

        let commit_message = if is_deleted {
            format!("chore: auto backup delete {}", relative_path.display())
        } else {
            format!("chore: auto backup {}", relative_path.display())
        };
        let (commit_tx, commit_rx) = oneshot();
        self.command_tx
            .send(GitCommand::CommitPath {
                path: relative_path.clone(),
                message: commit_message,
                reply: commit_tx,
            })
            .map_err(|source| Error::Watcher(format!("failed to send commit command: {source}")))?;
        let commit_result = commit_rx.recv().map_err(|source| {
            Error::Watcher(format!("failed to receive commit command result: {source}"))
        })?;
        let hash = commit_result?;
        info!(
            "created backup commit {} for {}",
            hash,
            relative_path.display()
        );
        Ok(())
    }

    fn should_suppress_storm(&self, relative_path: &Path) -> Result<bool> {
        let now = Instant::now();
        let mut guard = self
            .last_seen
            .lock()
            .map_err(|_| Error::Watcher("failed to lock storm suppression state".to_owned()))?;
        if let Some(last_seen) = guard.get_mut(relative_path) {
            let within_window =
                now.duration_since(*last_seen) < Duration::from_secs(self.config.storm_window_sec);
            *last_seen = now;
            return Ok(within_window);
        }
        guard.insert(relative_path.to_path_buf(), now);
        Ok(false)
    }
}

fn relative_to_workspace(workspace: &Path, absolute_path: &Path) -> PathBuf {
    absolute_path
        .strip_prefix(workspace)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| absolute_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::PersonaWatcher;
    use crate::config::WorkspaceConfig;
    use crate::executor::GitExecutor;
    use crate::git::GitRepo;
    use crate::state::{SharedSystemState, SystemState};
    use crate::types::WatchMode;

    #[test]
    fn test_storm_suppression_coalesces_rapid_same_file_changes() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo.clone());
        let state = SharedSystemState::new_running();
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Watcher,
        };
        let watcher = PersonaWatcher::new(&config, executor.sender(), state)
            .unwrap_or_else(|err| panic!("watcher creation failed: {err}"));

        fs::write(dir.path().join("SOUL.md"), "v1\n")
            .unwrap_or_else(|err| panic!("write failed: {err}"));
        assert!(watcher.handle_path(dir.path().join("SOUL.md")).is_ok());
        fs::write(dir.path().join("SOUL.md"), "v2\n")
            .unwrap_or_else(|err| panic!("write failed: {err}"));
        assert!(watcher.handle_path(dir.path().join("SOUL.md")).is_ok());

        let commit_count = git_commit_count(repo.path());
        assert_eq!(commit_count, 1);
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_pause_resume_and_drain_transitions() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo);
        let state = SharedSystemState::new_running();
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Watcher,
        };
        let watcher = PersonaWatcher::new(&config, executor.sender(), state.clone())
            .unwrap_or_else(|err| panic!("watcher creation failed: {err}"));

        assert!(watcher.pause().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Running),
            SystemState::Paused
        );
        assert!(watcher.resume().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Paused),
            SystemState::Running
        );
        assert!(watcher.drain().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Running),
            SystemState::Draining
        );
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_run_stops_when_stop_flag_set() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo);
        let state = SharedSystemState::new_running();
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Watcher,
        };

        let watcher = PersonaWatcher::new(&config, executor.sender(), state)
            .unwrap_or_else(|err| panic!("watcher creation failed: {err}"));
        let stop_handle = watcher.stop_handle();
        let worker = thread::spawn(move || watcher.run());

        thread::sleep(Duration::from_millis(100));
        stop_handle.store(true, Ordering::Relaxed);
        let join_result = worker.join();
        assert!(join_result.is_ok());
        let run_result = join_result.unwrap_or_else(|_| panic!("watcher thread panicked"));
        assert!(run_result.is_ok());
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_signal_file_in_hook_dir_is_ignored() {
        let dir = tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let repo = GitRepo::init(dir.path()).unwrap_or_else(|err| panic!("init failed: {err}"));
        let mut executor = GitExecutor::start(repo.clone());
        let state = SharedSystemState::new_running();
        let config = WorkspaceConfig {
            workspace: dir.path().to_path_buf(),
            watch: vec![".git-bak/hooks/*.json".to_owned()],
            debounce_ms: 10,
            push_interval_sec: 600,
            push_commit_threshold: 50,
            push_backoff_base_sec: 30,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Watcher,
        };

        let watcher = PersonaWatcher::new(&config, executor.sender(), state)
            .unwrap_or_else(|err| panic!("watcher creation failed: {err}"));
        let hook_dir = dir.path().join(".git-bak/hooks");
        fs::create_dir_all(&hook_dir).unwrap_or_else(|err| panic!("mkdir failed: {err}"));
        let signal_file = hook_dir.join("signal-01.json");
        fs::write(&signal_file, "{\"event\":\"command:new\"}\n")
            .unwrap_or_else(|err| panic!("write signal file failed: {err}"));

        assert!(watcher.handle_path(signal_file).is_ok());
        let commit_count = git_commit_count(repo.path());
        assert_eq!(commit_count, 0);
        assert!(executor.stop().is_ok());
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

        let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        raw.parse::<i64>().unwrap_or(0)
    }
}
