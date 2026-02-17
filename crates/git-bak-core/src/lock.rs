use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct ProcessLock {
    _file: File,
    path: PathBuf,
}

impl ProcessLock {
    pub fn acquire(repo_path: impl AsRef<Path>) -> Result<Self> {
        let lock_path = repo_path.as_ref().join(".git").join("git-bak.lock");
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

#[cfg(test)]
mod tests {
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
}
