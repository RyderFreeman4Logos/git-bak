pub mod config;
pub mod error;
pub mod filter;
pub mod git;
pub mod hash;
pub mod lock;
pub mod types;
pub mod watcher;

pub use config::WorkspaceConfig;
pub use error::{Error, Result};
pub use filter::PathFilter;
pub use git::GitRepo;
pub use hash::{file_hash, is_stable};
pub use lock::ProcessLock;
pub use types::{CommitStrategy, PushStrategy, WatchMode};
pub use watcher::PersonaWatcher;
