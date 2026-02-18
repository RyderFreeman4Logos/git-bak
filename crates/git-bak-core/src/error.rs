use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("watcher error: {0}")]
    Watcher(String),
    #[error("hook error: {0}")]
    Hook(String),
    #[error("lock error: {0}")]
    Lock(String),
}

pub type Result<T> = std::result::Result<T, Error>;
