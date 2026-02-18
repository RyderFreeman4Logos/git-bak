use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use tracing::{debug, info, warn};

use crate::config::WorkspaceConfig;
use crate::error::{Error, Result};
use crate::filter::PathFilter;
use crate::git::GitRepo;
use crate::hash::is_stable;

#[derive(Clone)]
pub struct PersonaWatcher {
    config: WorkspaceConfig,
    repo: GitRepo,
    stop_flag: Arc<AtomicBool>,
}

impl PersonaWatcher {
    pub fn new(config: &WorkspaceConfig, repo: GitRepo) -> Result<Self> {
        if !config.workspace.exists() {
            return Err(Error::Watcher(format!(
                "workspace path does not exist: {}",
                config.workspace.display()
            )));
        }

        Ok(Self {
            config: config.clone(),
            repo,
            stop_flag: Arc::new(AtomicBool::new(false)),
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

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_flag)
    }

    fn handle_debounce_result(&self, result: DebounceEventResult) -> Result<()> {
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
        let relative_path = relative_to_workspace(&self.config.workspace, &absolute_path);
        if PathFilter::matches_denylist(&relative_path) {
            debug!("skip denied path {}", relative_path.display());
            return Ok(());
        }

        if !PathFilter::matches_allowlist(&relative_path, &self.config.watch) {
            debug!("skip non-watched path {}", relative_path.display());
            return Ok(());
        }

        if !absolute_path.is_file() {
            return Ok(());
        }

        let stable = is_stable(&absolute_path, self.config.debounce_ms)?;
        if !stable {
            warn!("skip unstable path {}", absolute_path.display());
            return Ok(());
        }

        self.repo.add(relative_path.as_path())?;
        let status = self.repo.status()?;
        if status.is_empty() {
            return Ok(());
        }

        let commit_message = format!("chore: auto backup {}", relative_path.display());
        let hash = self
            .repo
            .commit_path(&commit_message, relative_path.as_path())?;
        info!(
            "created backup commit {} for {}",
            hash,
            relative_path.display()
        );
        Ok(())
    }
}

fn relative_to_workspace(workspace: &Path, absolute_path: &Path) -> PathBuf {
    absolute_path
        .strip_prefix(workspace)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| absolute_path.to_path_buf())
}
