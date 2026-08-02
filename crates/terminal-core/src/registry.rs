//! The set of live terminal sessions.

use crate::pty::{PtyConfig, PtyError, PtyEvent, PtySession};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tervin_core::PaneId;

/// Owns every live PTY, keyed by pane.
///
/// Sessions outlive the pane that created them only until explicitly closed, so
/// a detached or backgrounded task keeps running while its pane is hidden.
#[derive(Default)]
pub struct TerminalRegistry {
    sessions: RwLock<HashMap<PaneId, Arc<PtySession>>>,
}

impl TerminalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(
        &self,
        config: PtyConfig,
        sink: Arc<dyn Fn(PtyEvent) + Send + Sync>,
    ) -> Result<Arc<PtySession>, PtyError> {
        let pane_id = config.pane_id.clone();
        let session = Arc::new(PtySession::spawn(config, sink)?);
        self.sessions.write().insert(pane_id, session.clone());
        Ok(session)
    }

    pub fn get(&self, pane_id: &PaneId) -> Option<Arc<PtySession>> {
        self.sessions.read().get(pane_id).cloned()
    }

    pub fn write(&self, pane_id: &PaneId, data: &[u8]) -> Result<(), PtyError> {
        self.get(pane_id)
            .ok_or_else(|| PtyError::NotRunning(pane_id.clone()))?
            .write(data)
    }

    pub fn resize(&self, pane_id: &PaneId, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.get(pane_id)
            .ok_or_else(|| PtyError::NotRunning(pane_id.clone()))?
            .resize(cols, rows)
    }

    /// Terminate and forget a session.
    pub fn close(&self, pane_id: &PaneId) -> Result<(), PtyError> {
        if let Some(session) = self.sessions.write().remove(pane_id) {
            session.kill()?;
        }
        Ok(())
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.sessions.read().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.sessions.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop sessions whose child has exited, returning the panes reaped.
    pub fn reap(&self) -> Vec<PaneId> {
        let mut dead = Vec::new();
        self.sessions.write().retain(|pane_id, session| {
            if session.is_alive() {
                true
            } else {
                dead.push(pane_id.clone());
                false
            }
        });
        dead
    }
}
