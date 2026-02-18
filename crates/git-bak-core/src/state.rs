use std::sync::{Arc, RwLock};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemState {
    Running,
    Paused,
    Draining,
    ExclusiveOp(String),
}

#[derive(Debug, Clone)]
pub struct SharedSystemState {
    inner: Arc<RwLock<SystemState>>,
}

impl SharedSystemState {
    pub fn new_running() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SystemState::Running)),
        }
    }

    pub fn current(&self) -> Result<SystemState> {
        self.inner
            .read()
            .map(|state| state.clone())
            .map_err(|_| Error::Watcher("failed to read system state lock".to_owned()))
    }

    pub fn can_process_events(&self) -> bool {
        matches!(self.current(), Ok(SystemState::Running))
    }

    pub fn pause(&self) -> Result<()> {
        let mut state = self
            .inner
            .write()
            .map_err(|_| Error::Watcher("failed to lock system state for pause".to_owned()))?;
        match &*state {
            SystemState::Running => {
                *state = SystemState::Paused;
                Ok(())
            }
            _ => Err(Error::Watcher(format!(
                "invalid transition to Paused from {:?}",
                &*state
            ))),
        }
    }

    pub fn resume(&self) -> Result<()> {
        let mut state = self
            .inner
            .write()
            .map_err(|_| Error::Watcher("failed to lock system state for resume".to_owned()))?;
        match &*state {
            SystemState::Paused | SystemState::Draining => {
                *state = SystemState::Running;
                Ok(())
            }
            _ => Err(Error::Watcher(format!(
                "invalid transition to Running from {:?}",
                &*state
            ))),
        }
    }

    pub fn start_draining(&self) -> Result<()> {
        let mut state = self
            .inner
            .write()
            .map_err(|_| Error::Watcher("failed to lock system state for draining".to_owned()))?;
        match &*state {
            SystemState::Running | SystemState::Paused => {
                *state = SystemState::Draining;
                Ok(())
            }
            _ => Err(Error::Watcher(format!(
                "invalid transition to Draining from {:?}",
                &*state
            ))),
        }
    }

    pub fn enter_exclusive(&self, op_name: impl Into<String>) -> Result<()> {
        let mut state = self.inner.write().map_err(|_| {
            Error::Watcher("failed to lock system state for exclusive operation".to_owned())
        })?;
        match &*state {
            SystemState::Running => {
                *state = SystemState::ExclusiveOp(op_name.into());
                Ok(())
            }
            _ => Err(Error::Watcher(format!(
                "invalid transition to ExclusiveOp from {:?}",
                &*state
            ))),
        }
    }

    pub fn exit_exclusive(&self) -> Result<()> {
        let mut state = self.inner.write().map_err(|_| {
            Error::Watcher("failed to lock system state for exclusive operation exit".to_owned())
        })?;
        match &*state {
            SystemState::ExclusiveOp(_) => {
                *state = SystemState::Running;
                Ok(())
            }
            _ => Err(Error::Watcher(format!(
                "invalid transition from {:?} via exit_exclusive",
                &*state
            ))),
        }
    }
}

impl Default for SharedSystemState {
    fn default() -> Self {
        Self::new_running()
    }
}

#[cfg(test)]
mod tests {
    use super::{SharedSystemState, SystemState};

    #[test]
    fn test_state_allows_valid_transitions() {
        let state = SharedSystemState::new_running();
        assert!(state.pause().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Running),
            SystemState::Paused
        );
        assert!(state.resume().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Paused),
            SystemState::Running
        );
        assert!(state.start_draining().is_ok());
        assert_eq!(
            state.current().unwrap_or(SystemState::Running),
            SystemState::Draining
        );
        assert!(state.resume().is_ok());
        assert!(state.enter_exclusive("checkpoint").is_ok());
        assert!(state.exit_exclusive().is_ok());
    }

    #[test]
    fn test_state_rejects_invalid_transitions() {
        let state = SharedSystemState::new_running();
        assert!(state.resume().is_err());
        assert!(state.exit_exclusive().is_err());
        assert!(state.enter_exclusive("bisect").is_ok());
        assert!(state.pause().is_err());
        assert!(state.start_draining().is_err());
    }
}
