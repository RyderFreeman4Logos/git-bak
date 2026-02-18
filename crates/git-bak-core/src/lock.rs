use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct ProcessLock {
    _file: File,
    path: PathBuf,
}

impl ProcessLock {
    pub fn acquire(repo_path: impl AsRef<Path>) -> Result<Self> {
        let repo_path = repo_path.as_ref();
        let lock_path = lock_path_for_repo(repo_path)?;
        let parent = lock_path.parent().ok_or_else(|| {
            Error::Lock(format!(
                "failed to determine parent directory for {}",
                lock_path.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|source| {
            Error::Lock(format!(
                "failed to create lock directory {}: {source}",
                parent.display()
            ))
        })?;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| {
                Error::Lock(format!(
                    "failed to open lock file {}: {source}",
                    lock_path.display()
                ))
            })?;

        file.try_lock_exclusive().map_err(|source| {
            Error::Lock(format!(
                "failed to acquire lock {}: {source}",
                lock_path.display()
            ))
        })?;

        Ok(Self {
            _file: file,
            path: lock_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn lock_path_for_repo(repo_path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-parse", "--git-path", "git-bak.lock"])
        .output()
        .map_err(|source| {
            Error::Lock(format!(
                "failed to resolve git lock path for {}: {source}",
                repo_path.display()
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::Lock(format!(
            "failed to resolve git lock path for {}: {}",
            repo_path.display(),
            stderr
        )));
    }

    let raw_path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if raw_path.is_empty() {
        return Err(Error::Lock(format!(
            "failed to resolve git lock path for {}: empty response",
            repo_path.display()
        )));
    }

    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo_path.join(path))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use crate::git::GitRepo;

    use super::ProcessLock;

    #[test]
    fn test_acquire_fails_when_lock_is_already_held() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };

        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init repository: {err}"),
        };

        let first = ProcessLock::acquire(repo.path());
        assert!(first.is_ok());

        let second = ProcessLock::acquire(repo.path());
        assert!(second.is_err());
    }

    #[test]
    fn test_acquire_succeeds_for_git_worktree() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };

        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init repository: {err}"),
        };
        setup_git_identity(repo.path());

        let seed_file = dir.path().join("SOUL.md");
        assert!(fs::write(&seed_file, "seed\n").is_ok());
        assert!(repo.add(Path::new("SOUL.md")).is_ok());
        assert!(repo.commit("seed commit").is_ok());

        let worktree_path = dir.path().join("wt");
        let worktree_status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["worktree", "add", "-b", "wt-lock-test"])
            .arg(&worktree_path)
            .status();
        match worktree_status {
            Ok(status) => assert!(status.success()),
            Err(err) => panic!("failed to create worktree: {err}"),
        }

        let lock = ProcessLock::acquire(&worktree_path);
        assert!(lock.is_ok());
    }

    fn setup_git_identity(repo_path: &Path) {
        let name_status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["config", "user.name", "git-bak-test"])
            .status();
        match name_status {
            Ok(status) => assert!(status.success()),
            Err(err) => panic!("failed to configure user.name: {err}"),
        }

        let email_status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["config", "user.email", "git-bak@example.com"])
            .status();
        match email_status {
            Ok(status) => assert!(status.success()),
            Err(err) => panic!("failed to configure user.email: {err}"),
        }
    }
}
