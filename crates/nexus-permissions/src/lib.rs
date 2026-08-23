//! Permission decisions are explicit and inspectable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
    Sandbox,
}

pub trait PermissionPolicy: Send + Sync {
    fn evaluate(&self, action: &str) -> PermissionDecision;
}

#[derive(Debug, Default)]
pub struct AskByDefault;

impl PermissionPolicy for AskByDefault {
    fn evaluate(&self, _action: &str) -> PermissionDecision {
        PermissionDecision::Ask
    }
}
