use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct GitRepo {
    path: PathBuf,
}

impl GitRepo {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let repo_path = path.as_ref().to_path_buf();
        let git_dir = repo_path.join(".git");
        if !git_dir.exists() {
            return Err(Error::Git(format!(
                "missing .git directory in {}",
                repo_path.display()
            )));
        }

        Ok(Self { path: repo_path })
    }

    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        let repo_path = path.as_ref();
        fs::create_dir_all(repo_path).map_err(|source| {
            Error::Git(format!(
                "failed to create repository directory {}: {source}",
                repo_path.display()
            ))
        })?;

        run_git_in(repo_path, ["init"])?;
        ensure_local_identity(repo_path)?;
        Self::new(repo_path)
    }

    pub fn status(&self) -> Result<String> {
        run_git_in(&self.path, ["status", "--porcelain"])
    }

    pub fn add(&self, file: &Path) -> Result<()> {
        let relative_path = if file.is_absolute() {
            file.strip_prefix(&self.path).map_err(|source| {
                Error::Git(format!(
                    "path {} is outside repository {}: {source}",
                    file.display(),
                    self.path.display()
                ))
            })?
        } else {
            file
        };

        run_git_in_os(
            &self.path,
            [
                OsStr::new("add"),
                OsStr::new("--"),
                relative_path.as_os_str(),
            ],
        )
        .map(|_| ())
    }

    pub fn add_all(&self, file: &Path) -> Result<()> {
        let relative_path = if file.is_absolute() {
            file.strip_prefix(&self.path).map_err(|source| {
                Error::Git(format!(
                    "path {} is outside repository {}: {source}",
                    file.display(),
                    self.path.display()
                ))
            })?
        } else {
            file
        };

        run_git_in_os(
            &self.path,
            [
                OsStr::new("add"),
                OsStr::new("-A"),
                OsStr::new("--"),
                relative_path.as_os_str(),
            ],
        )
        .map(|_| ())
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        run_git_in_os(
            &self.path,
            [
                OsStr::new("commit"),
                OsStr::new("-m"),
                OsStr::new(message),
                OsStr::new("--no-gpg-sign"),
            ],
        )?;
        run_git_in(&self.path, ["rev-parse", "HEAD"]).map(|hash| hash.trim().to_owned())
    }

    pub fn commit_path(&self, message: &str, path: &Path) -> Result<String> {
        let relative_path = if path.is_absolute() {
            path.strip_prefix(&self.path).map_err(|source| {
                Error::Git(format!(
                    "path {} is outside repository {}: {source}",
                    path.display(),
                    self.path.display()
                ))
            })?
        } else {
            path
        };

        run_git_in_os(
            &self.path,
            [
                OsStr::new("commit"),
                OsStr::new("-m"),
                OsStr::new(message),
                OsStr::new("--no-gpg-sign"),
                OsStr::new("--"),
                relative_path.as_os_str(),
            ],
        )?;
        run_git_in(&self.path, ["rev-parse", "HEAD"]).map(|hash| hash.trim().to_owned())
    }

    pub fn has_head_commit(&self) -> Result<bool> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .map_err(|source| {
                Error::Git(format!(
                    "failed to verify HEAD in {}: {source}",
                    self.path.display()
                ))
            })?;
        Ok(output.status.success())
    }

    pub fn recent_commits(&self, limit: usize) -> Result<String> {
        if limit == 0 || !self.has_head_commit()? {
            return Ok(String::new());
        }
        let count_arg = format!("-{limit}");
        run_git_in_os(
            &self.path,
            [
                OsStr::new("log"),
                OsStr::new("--oneline"),
                OsStr::new(count_arg.as_str()),
            ],
        )
    }

    pub fn tag(&self, name: &str) -> Result<()> {
        run_git_in_os(&self.path, [OsStr::new("tag"), OsStr::new(name)]).map(|_| ())
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        run_git_in_os(&self.path, [OsStr::new("checkout"), OsStr::new(name)]).map(|_| ())
    }

    pub fn create_branch(&self, name: &str, start_point: Option<&str>) -> Result<()> {
        match start_point {
            Some(start_point) => run_git_in_os(
                &self.path,
                [
                    OsStr::new("branch"),
                    OsStr::new(name),
                    OsStr::new(start_point),
                ],
            )
            .map(|_| ()),
            None => run_git_in_os(&self.path, [OsStr::new("branch"), OsStr::new(name)]).map(|_| ()),
        }
    }

    pub fn merge_squash(&self, from: &str) -> Result<()> {
        run_git_in_os(
            &self.path,
            [
                OsStr::new("merge"),
                OsStr::new("--squash"),
                OsStr::new(from),
            ],
        )
        .map(|_| ())
    }

    pub fn current_branch(&self) -> Result<String> {
        run_git_in(&self.path, ["rev-parse", "--abbrev-ref", "HEAD"])
    }

    pub fn push(&self, remote: &str) -> Result<()> {
        run_git_in_os(
            &self.path,
            [
                OsStr::new("push"),
                OsStr::new(remote),
                OsStr::new("HEAD"),
            ],
        )
        .map(|_| ())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn run_git_args(&self, args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<String> {
        run_git_in(&self.path, args)
    }

    pub fn run_git_os_args(
        &self,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<String> {
        run_git_in_os(&self.path, args)
    }
}

fn run_git_in(path: &Path, args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<String> {
    let arguments: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(&arguments)
        .output()
        .map_err(|source| {
            Error::Git(format!(
                "failed to run git {:?} in {}: {source}",
                arguments,
                path.display()
            ))
        })?;

    if output.status.success() {
        let stdout = trim_trailing_newlines(String::from_utf8_lossy(&output.stdout).as_ref());
        return Ok(stdout);
    }

    let stderr = trim_trailing_newlines(String::from_utf8_lossy(&output.stderr).as_ref());
    Err(Error::Git(format!(
        "git {:?} failed in {}: {}",
        arguments,
        path.display(),
        stderr
    )))
}

fn ensure_local_identity(path: &Path) -> Result<()> {
    if !has_git_config_value(path, "user.name")? {
        run_git_in(path, ["config", "--local", "user.name", "git-bak"])?;
    }
    if !has_git_config_value(path, "user.email")? {
        run_git_in(
            path,
            ["config", "--local", "user.email", "git-bak@localhost"],
        )?;
    }
    Ok(())
}

fn has_git_config_value(path: &Path, key: &str) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["config", "--get", key])
        .output()
        .map_err(|source| {
            Error::Git(format!(
                "failed to read git config {} in {}: {source}",
                key,
                path.display()
            ))
        })?;

    if !output.status.success() {
        return Ok(false);
    }
    let value = trim_trailing_newlines(String::from_utf8_lossy(&output.stdout).as_ref());
    Ok(!value.is_empty())
}

fn run_git_in_os(path: &Path, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Result<String> {
    let arguments: Vec<std::ffi::OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(&arguments)
        .output()
        .map_err(|source| {
            Error::Git(format!(
                "failed to run git command in {}: {source}",
                path.display()
            ))
        })?;

    if output.status.success() {
        let stdout = trim_trailing_newlines(String::from_utf8_lossy(&output.stdout).as_ref());
        return Ok(stdout);
    }

    let stderr = trim_trailing_newlines(String::from_utf8_lossy(&output.stderr).as_ref());
    Err(Error::Git(format!(
        "git command {:?} failed in {}: {}",
        arguments,
        path.display(),
        stderr
    )))
}

fn trim_trailing_newlines(value: &str) -> String {
    value.trim_end_matches(['\n', '\r']).to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::GitRepo;

    #[test]
    fn test_new_fails_without_git_directory() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };

        let result = GitRepo::new(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_init_add_commit_and_status_clean() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };
        setup_git_identity(repo.path());

        let file_path = dir.path().join("SOUL.md");
        let write_result = fs::write(&file_path, "hello world\n");
        assert!(write_result.is_ok());

        let add_result = repo.add(Path::new("SOUL.md"));
        assert!(add_result.is_ok());

        let commit_result = repo.commit("initial backup");
        assert!(commit_result.is_ok());

        let status = match repo.status() {
            Ok(status) => status,
            Err(err) => panic!("failed to read status: {err}"),
        };
        assert!(status.is_empty());
    }

    #[test]
    fn test_commit_path_only_commits_target_file() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };
        setup_git_identity(repo.path());

        let soul_path = dir.path().join("SOUL.md");
        let user_path = dir.path().join("USER.md");
        assert!(fs::write(&soul_path, "soul\n").is_ok());
        assert!(fs::write(&user_path, "user\n").is_ok());
        assert!(repo.add(Path::new("SOUL.md")).is_ok());
        assert!(repo.add(Path::new("USER.md")).is_ok());

        let commit_result = repo.commit_path("commit soul only", Path::new("SOUL.md"));
        assert!(commit_result.is_ok());

        let status = repo.status().unwrap_or_default();
        assert!(status.contains("A  USER.md"));
        assert!(!status.contains("SOUL.md"));
    }

    #[test]
    fn test_add_all_stages_deletion_for_target_path() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };
        setup_git_identity(repo.path());

        let file_path = dir.path().join("SOUL.md");
        assert!(fs::write(&file_path, "seed\n").is_ok());
        assert!(repo.add(Path::new("SOUL.md")).is_ok());
        assert!(repo.commit("seed").is_ok());

        assert!(fs::remove_file(&file_path).is_ok());
        assert!(repo.add_all(Path::new("SOUL.md")).is_ok());

        let status = repo.status().unwrap_or_default();
        assert!(status.contains("D  SOUL.md"));
    }

    #[test]
    fn test_recent_commits_is_empty_when_repository_has_no_commits() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };

        let has_commit = repo.has_head_commit();
        assert!(has_commit.is_ok());
        assert!(!has_commit.unwrap_or(true));

        let commits = repo.recent_commits(5);
        assert!(commits.is_ok());
        assert!(commits.unwrap_or_default().is_empty());
    }

    #[test]
    fn test_status_preserves_porcelain_leading_space() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };
        setup_git_identity(repo.path());

        let file_path = dir.path().join("SOUL.md");
        assert!(fs::write(&file_path, "before\n").is_ok());
        assert!(repo.add(Path::new("SOUL.md")).is_ok());
        assert!(repo.commit("seed").is_ok());

        assert!(fs::write(&file_path, "after\n").is_ok());
        let status = repo.status().unwrap_or_default();
        assert!(status.starts_with(" M SOUL.md"));
    }

    #[test]
    fn test_init_sets_identity_so_commit_works_without_manual_config() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let repo = match GitRepo::init(dir.path()) {
            Ok(repo) => repo,
            Err(err) => panic!("failed to init git repo: {err}"),
        };

        let file_path = dir.path().join("SOUL.md");
        assert!(fs::write(&file_path, "seed\n").is_ok());
        assert!(repo.add(Path::new("SOUL.md")).is_ok());
        assert!(repo.commit("seed").is_ok());
    }

    fn setup_git_identity(repo_path: &Path) {
        let name_status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["config", "user.name", "git-bak-test"])
            .status();
        match name_status {
            Ok(status) => assert!(status.success()),
            Err(err) => panic!("failed to set git user.name: {err}"),
        }

        let email_status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["config", "user.email", "git-bak@example.com"])
            .status();
        match email_status {
            Ok(status) => assert!(status.success()),
            Err(err) => panic!("failed to set git user.email: {err}"),
        }
    }
}
