//! Stable domain contracts shared across the Nexus runtime.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub prompt: String,
}

impl Task {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), prompt: prompt.into() }
    }
}
