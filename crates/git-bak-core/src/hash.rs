use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};

pub fn file_hash(path: &Path) -> Result<blake3::Hash> {
    let bytes = fs::read(path).map_err(|source| {
        Error::Watcher(format!("failed to read file {}: {source}", path.display()))
    })?;
    Ok(blake3::hash(&bytes))
}

pub fn is_stable(path: &Path, delay_ms: u64) -> Result<bool> {
    let first = file_hash(path)?;
    thread::sleep(Duration::from_millis(delay_ms));
    let second = file_hash(path)?;
    Ok(first == second)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::is_stable;

    #[test]
    fn test_is_stable_with_same_content_returns_true() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let path = dir.path().join("MEMORY.md");
        let write_result = fs::write(&path, "consistent content");
        assert!(write_result.is_ok());

        let stable = match is_stable(&path, 25) {
            Ok(stable) => stable,
            Err(err) => panic!("failed to evaluate stability: {err}"),
        };
        assert!(stable);
    }

    #[test]
    fn test_is_stable_with_content_change_returns_false() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let path = Arc::new(dir.path().join("MEMORY.md"));
        let write_result = fs::write(path.as_path(), "before");
        assert!(write_result.is_ok());

        let writer_path = Arc::clone(&path);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            let update_result = fs::write(writer_path.as_path(), "after");
            assert!(update_result.is_ok());
        });

        let stable = match is_stable(path.as_path(), 30) {
            Ok(stable) => stable,
            Err(err) => panic!("failed to evaluate stability: {err}"),
        };
        assert!(!stable);

        let join_result = handle.join();
        assert!(join_result.is_ok());
    }
}
