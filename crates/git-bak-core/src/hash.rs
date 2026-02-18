use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};

const MAX_HASH_FILE_BYTES: u64 = 5 * 1024 * 1024;

pub fn file_hash(path: &Path) -> Result<blake3::Hash> {
    let mut file = File::open(path).map_err(|source| {
        Error::Watcher(format!("failed to read file {}: {source}", path.display()))
    })?;
    let size = file.metadata().map_err(|source| {
        Error::Watcher(format!(
            "failed to read metadata for {}: {source}",
            path.display()
        ))
    })?;
    if size.len() > MAX_HASH_FILE_BYTES {
        return Err(Error::Watcher(format!(
            "refusing to hash large file {} ({} bytes > {} bytes)",
            path.display(),
            size.len(),
            MAX_HASH_FILE_BYTES
        )));
    }

    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| {
            Error::Watcher(format!("failed to read file {}: {source}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
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
    use std::fs::File;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{MAX_HASH_FILE_BYTES, file_hash, is_stable};

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

    #[test]
    fn test_file_hash_rejects_large_file() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let path = dir.path().join("big.bin");
        let file = File::create(&path);
        assert!(file.is_ok());
        let file = file.unwrap_or_else(|_| unreachable!());
        let set_len_result = file.set_len(MAX_HASH_FILE_BYTES + 1);
        assert!(set_len_result.is_ok());

        let hash_result = file_hash(&path);
        assert!(hash_result.is_err());
    }
}
