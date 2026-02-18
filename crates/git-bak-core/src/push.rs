use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::config::WorkspaceConfig;
use crate::error::{Error, Result};
use crate::executor::{GitCommand, oneshot};

const DEFAULT_REMOTE: &str = "origin";
const MAX_BACKOFF_SEC: u64 = 15 * 60;

pub struct PushScheduler {
    stop_flag: Arc<AtomicBool>,
    worker_thread: Thread,
    worker: Option<JoinHandle<()>>,
}

struct PushLoopContext {
    repo_path: PathBuf,
    command_tx: mpsc::Sender<GitCommand>,
    commit_count: Arc<AtomicUsize>,
    stop_flag: Arc<AtomicBool>,
    push_interval: Duration,
    threshold: usize,
    base_backoff: Duration,
    remote: String,
}

impl PushScheduler {
    pub fn start(
        config: &WorkspaceConfig,
        command_tx: mpsc::Sender<GitCommand>,
        commit_count: Arc<AtomicUsize>,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let worker_stop_flag = Arc::clone(&stop_flag);
        let repo_path = config.workspace.clone();
        let push_interval = Duration::from_secs(config.push_interval_sec.max(1));
        let threshold = config.push_commit_threshold.max(1);
        let base_backoff = Duration::from_secs(config.push_backoff_base_sec.max(1));
        let worker = thread::spawn(move || {
            run_push_loop(PushLoopContext {
                repo_path,
                command_tx,
                commit_count,
                stop_flag: worker_stop_flag,
                push_interval,
                threshold,
                base_backoff,
                remote: DEFAULT_REMOTE.to_owned(),
            })
        });
        let worker_thread = worker.thread().clone();

        Self {
            stop_flag,
            worker_thread,
            worker: Some(worker),
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.worker.is_none() {
            return Ok(());
        }
        self.stop_flag.store(true, Ordering::Relaxed);
        self.worker_thread.unpark();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::Watcher("push scheduler thread panicked".to_owned()))?;
        }
        Ok(())
    }
}

impl Drop for PushScheduler {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_push_loop(context: PushLoopContext) {
    let PushLoopContext {
        repo_path,
        command_tx,
        commit_count,
        stop_flag,
        push_interval,
        threshold,
        base_backoff,
        remote,
    } = context;
    let max_backoff = Duration::from_secs(MAX_BACKOFF_SEC);
    let mut backoff = base_backoff;
    let mut next_interval_deadline = Instant::now() + push_interval;
    let mut next_retry_deadline = Instant::now();

    while !stop_flag.load(Ordering::Relaxed) {
        thread::park_timeout(Duration::from_secs(1));
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let now = Instant::now();
        let commits = commit_count.load(Ordering::Relaxed);
        if commits == 0 {
            next_interval_deadline = now + push_interval;
            continue;
        }
        let timer_ready = now >= next_interval_deadline;
        let threshold_ready = commits >= threshold;
        if !(timer_ready || threshold_ready) {
            continue;
        }
        if now < next_retry_deadline {
            continue;
        }

        if !is_network_reachable(repo_path.as_path(), &remote) {
            warn!("skip push: remote {} is unreachable", remote);
            next_retry_deadline = now + backoff;
            backoff = next_backoff(backoff, max_backoff);
            continue;
        }

        match push_once(&command_tx, &remote) {
            Ok(()) => {
                debug!("push succeeded, resetting commit counter");
                commit_count.store(0, Ordering::Relaxed);
                backoff = base_backoff;
                next_retry_deadline = now;
                next_interval_deadline = now + push_interval;
            }
            Err(err) => {
                warn!("push failed: {err}");
                next_retry_deadline = now + backoff;
                backoff = next_backoff(backoff, max_backoff);
            }
        }
    }
}

fn push_once(command_tx: &mpsc::Sender<GitCommand>, remote: &str) -> Result<()> {
    let (push_tx, push_rx) = oneshot();
    command_tx
        .send(GitCommand::Push {
            remote: remote.to_owned(),
            reply: push_tx,
        })
        .map_err(|source| Error::Git(format!("failed to enqueue push command: {source}")))?;
    push_rx
        .recv()
        .map_err(|source| Error::Git(format!("failed to receive push command result: {source}")))?
}

fn is_network_reachable(repo_path: &Path, remote: &str) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["ls-remote", "--exit-code", remote])
        .output();
    match output {
        Ok(output) => {
            if output.status.success() {
                return true;
            }
            if output.status.code() == Some(2) {
                let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
                return !(stderr.contains("could not resolve host")
                    || stderr.contains("could not read from remote repository")
                    || stderr.contains("does not appear to be a git repository")
                    || stderr.contains("repository not found"));
            }
            false
        }
        Err(_) => false,
    }
}

fn next_backoff(current: Duration, max_backoff: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > max_backoff {
        max_backoff
    } else {
        doubled
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::{PushScheduler, next_backoff};
    use crate::config::WorkspaceConfig;
    use crate::executor::{GitCommand, GitExecutor, oneshot};
    use crate::git::GitRepo;
    use crate::types::WatchMode;

    #[test]
    fn test_backoff_doubles_and_caps() {
        let max_backoff = Duration::from_secs(60);
        assert_eq!(
            next_backoff(Duration::from_secs(5), max_backoff).as_secs(),
            10
        );
        assert_eq!(
            next_backoff(Duration::from_secs(40), max_backoff).as_secs(),
            60
        );
        assert_eq!(
            next_backoff(Duration::from_secs(60), max_backoff).as_secs(),
            60
        );
    }

    #[test]
    fn test_timer_fires_after_interval() {
        let (repo, mut executor, mut scheduler) = setup_push_env(2, 99, 1, true);
        create_commit_via_executor(executor.sender(), repo.path(), "SOUL.md", "timer push");

        let pushed = wait_for_remote_commits(repo.path(), 1, Duration::from_secs(5));
        assert!(pushed);
        assert_eq!(executor.commit_count().load(Ordering::Relaxed), 0);
        assert!(scheduler.stop().is_ok());
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_network_unreachable_skips_push() {
        let (repo, mut executor, mut scheduler) = setup_push_env(1, 1, 1, false);
        create_commit_via_executor(executor.sender(), repo.path(), "SOUL.md", "no network");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if executor.commit_count().load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(executor.commit_count().load(Ordering::Relaxed) > 0);
        assert!(scheduler.stop().is_ok());
        assert!(executor.stop().is_ok());
    }

    #[test]
    fn test_commit_threshold_triggers_early_push() {
        let (repo, mut executor, mut scheduler) = setup_push_env(120, 1, 1, true);
        create_commit_via_executor(executor.sender(), repo.path(), "SOUL.md", "threshold push");

        let pushed = wait_for_remote_commits(repo.path(), 1, Duration::from_secs(5));
        assert!(pushed);
        assert_eq!(executor.commit_count().load(Ordering::Relaxed), 0);
        assert!(scheduler.stop().is_ok());
        assert!(executor.stop().is_ok());
    }

    fn setup_push_env(
        push_interval_sec: u64,
        threshold: usize,
        backoff_base_sec: u64,
        reachable_remote: bool,
    ) -> (GitRepo, GitExecutor, PushScheduler) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("git-bak-push-test-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap_or_else(|err| panic!("mkdir root failed: {err}"));
        let repo_path = root.join("repo");
        fs::create_dir_all(&repo_path).unwrap_or_else(|err| panic!("mkdir repo failed: {err}"));
        let repo =
            GitRepo::init(&repo_path).unwrap_or_else(|err| panic!("init repo failed: {err}"));

        if reachable_remote {
            let remote_path = root.join("remote.git");
            init_bare_remote(&remote_path);
            set_remote(repo.path(), "origin", &remote_path);
        } else {
            set_remote(
                repo.path(),
                "origin",
                Path::new("/tmp/non-existent-git-bak-remote"),
            );
        }

        let config = WorkspaceConfig {
            workspace: repo.path().to_path_buf(),
            watch: vec!["SOUL.md".to_owned()],
            debounce_ms: 10,
            push_interval_sec,
            push_commit_threshold: threshold,
            push_backoff_base_sec: backoff_base_sec,
            storm_window_sec: 5,
            hook_signal_dir: Path::new(".git-bak/hooks").to_path_buf(),
            mode: WatchMode::Watcher,
        };
        let executor = GitExecutor::start(repo.clone());
        let scheduler = PushScheduler::start(&config, executor.sender(), executor.commit_count());
        (repo, executor, scheduler)
    }

    fn init_bare_remote(path: &Path) {
        let status = Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(path)
            .status()
            .unwrap_or_else(|err| panic!("failed to init bare remote: {err}"));
        assert!(status.success());
    }

    fn set_remote(repo_path: &Path, remote: &str, remote_path: &Path) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args([
                "remote",
                "add",
                remote,
                remote_path.to_string_lossy().as_ref(),
            ])
            .status()
            .unwrap_or_else(|err| panic!("failed to set remote: {err}"));
        assert!(status.success());
    }

    fn create_commit_via_executor(
        command_tx: mpsc::Sender<GitCommand>,
        repo_path: &Path,
        relative_path: &str,
        message: &str,
    ) {
        fs::write(repo_path.join(relative_path), format!("{message}\n"))
            .unwrap_or_else(|err| panic!("write file failed: {err}"));
        let (add_tx, add_rx) = oneshot();
        command_tx
            .send(GitCommand::Add {
                path: Path::new(relative_path).to_path_buf(),
                reply: add_tx,
            })
            .unwrap_or_else(|err| panic!("send add command failed: {err}"));
        let add_result = add_rx
            .recv()
            .unwrap_or_else(|err| panic!("recv add command failed: {err}"));
        assert!(add_result.is_ok());

        let (commit_tx, commit_rx) = oneshot();
        command_tx
            .send(GitCommand::Commit {
                message: message.to_owned(),
                reply: commit_tx,
            })
            .unwrap_or_else(|err| panic!("send commit command failed: {err}"));
        let commit_result = commit_rx
            .recv()
            .unwrap_or_else(|err| panic!("recv commit command failed: {err}"));
        assert!(commit_result.is_ok());
    }

    fn wait_for_remote_commits(repo_path: &Path, expected: i64, timeout: Duration) -> bool {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["remote", "get-url", "origin"])
            .output();
        let output = match output {
            Ok(output) => output,
            Err(_) => return false,
        };
        if !output.status.success() {
            return false;
        }
        let remote_path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let start = Instant::now();
        while start.elapsed() <= timeout {
            let count = remote_commit_count(Path::new(&remote_path));
            if count >= expected {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn remote_commit_count(remote_path: &Path) -> i64 {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(remote_path)
            .args(["rev-list", "--count", "--all"])
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
