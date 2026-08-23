//! Storage contracts for sessions and execution history.

use async_trait::async_trait;
use nexus_core::SessionId;

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, id: SessionId) -> Result<(), StorageError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage error: {0}")]
    Backend(String),
}
