//! Permission decisions are explicit, policy-driven, and inspectable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
    Sandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub action: String,
}

impl PermissionRequest {
    #[must_use]
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            action: action.into(),
        }
    }
}

pub trait PermissionPolicy: Send + Sync {
    fn evaluate(&self, request: &PermissionRequest) -> PermissionDecision;
}

/// Human or application boundary used when a policy returns `ask`.
///
/// The permission crate deliberately does not know whether approval comes from
/// a CLI prompt, desktop dialog, IDE notification, or remote policy service.
pub trait PermissionApprover: Send + Sync {
    fn approve(&self, request: &PermissionRequest) -> bool;
}

#[derive(Debug, Default)]
pub struct AskByDefault;

impl PermissionPolicy for AskByDefault {
    fn evaluate(&self, _request: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Ask
    }
}

#[derive(Debug, Clone)]
pub struct RuleBasedPolicy {
    default: PermissionDecision,
    rules: HashMap<String, PermissionDecision>,
}

impl RuleBasedPolicy {
    #[must_use]
    pub fn new(default: PermissionDecision) -> Self {
        Self {
            default,
            rules: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_rule(mut self, action: impl Into<String>, decision: PermissionDecision) -> Self {
        self.rules.insert(action.into(), decision);
        self
    }
}

impl PermissionPolicy for RuleBasedPolicy {
    fn evaluate(&self, request: &PermissionRequest) -> PermissionDecision {
        if let Some(decision) = self.rules.get(&request.action) {
            return *decision;
        }

        self.rules
            .iter()
            .filter_map(|(rule, decision)| {
                rule.strip_suffix(".*")
                    .map(|prefix| format!("{prefix}."))
                    .filter(|prefix| request.action.starts_with(prefix))
                    .map(|prefix| (prefix.len(), *decision))
            })
            .max_by_key(|(length, _)| *length)
            .map_or(self.default, |(_, decision)| decision)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    #[error("permission denied for action: {0}")]
    Denied(String),
    #[error("approval required for action: {0}")]
    ApprovalRequired(String),
    #[error("approval denied for action: {0}")]
    ApprovalDenied(String),
    #[error("sandbox execution required for action: {0}")]
    SandboxRequired(String),
}

pub fn enforce(
    policy: &dyn PermissionPolicy,
    request: &PermissionRequest,
) -> Result<PermissionDecision, PermissionError> {
    match policy.evaluate(request) {
        PermissionDecision::Allow => Ok(PermissionDecision::Allow),
        PermissionDecision::Ask => Err(PermissionError::ApprovalRequired(request.action.clone())),
        PermissionDecision::Deny => Err(PermissionError::Denied(request.action.clone())),
        PermissionDecision::Sandbox => Err(PermissionError::SandboxRequired(request.action.clone())),
    }
}

pub fn enforce_with_approver(
    policy: &dyn PermissionPolicy,
    approver: Option<&dyn PermissionApprover>,
    request: &PermissionRequest,
) -> Result<PermissionDecision, PermissionError> {
    match policy.evaluate(request) {
        PermissionDecision::Allow => Ok(PermissionDecision::Allow),
        PermissionDecision::Ask => match approver {
            Some(approver) if approver.approve(request) => Ok(PermissionDecision::Allow),
            Some(_) => Err(PermissionError::ApprovalDenied(request.action.clone())),
            None => Err(PermissionError::ApprovalRequired(request.action.clone())),
        },
        PermissionDecision::Deny => Err(PermissionError::Denied(request.action.clone())),
        PermissionDecision::Sandbox => Err(PermissionError::SandboxRequired(request.action.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionApprover, PermissionDecision, PermissionError, PermissionPolicy, PermissionRequest,
        RuleBasedPolicy, enforce, enforce_with_approver,
    };

    struct StaticApprover(bool);

    impl PermissionApprover for StaticApprover {
        fn approve(&self, _request: &PermissionRequest) -> bool {
            self.0
        }
    }

    #[test]
    fn exact_rule_wins_over_default() {
        let policy = RuleBasedPolicy::new(PermissionDecision::Ask)
            .with_rule("filesystem.read", PermissionDecision::Allow);
        let request = PermissionRequest::new("filesystem.read");

        assert_eq!(policy.evaluate(&request), PermissionDecision::Allow);
    }

    #[test]
    fn wildcard_rule_matches_tool_family() {
        let policy = RuleBasedPolicy::new(PermissionDecision::Deny)
            .with_rule("filesystem.*", PermissionDecision::Allow);
        let request = PermissionRequest::new("filesystem.read");

        assert_eq!(policy.evaluate(&request), PermissionDecision::Allow);
    }

    #[test]
    fn ask_is_not_silently_allowed() {
        let policy = RuleBasedPolicy::new(PermissionDecision::Ask);
        let error = enforce(&policy, &PermissionRequest::new("shell.execute"))
            .expect_err("ask must require explicit approval");

        assert!(matches!(error, PermissionError::ApprovalRequired(_)));
    }

    #[test]
    fn approver_can_allow_asked_action() {
        let policy = RuleBasedPolicy::new(PermissionDecision::Ask);
        let request = PermissionRequest::new("shell.execute");

        assert_eq!(
            enforce_with_approver(&policy, Some(&StaticApprover(true)), &request)
                .expect("approved action should execute"),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn approver_can_reject_asked_action() {
        let policy = RuleBasedPolicy::new(PermissionDecision::Ask);
        let request = PermissionRequest::new("shell.execute");
        let error = enforce_with_approver(&policy, Some(&StaticApprover(false)), &request)
            .expect_err("rejected action should fail");

        assert!(matches!(error, PermissionError::ApprovalDenied(_)));
    }
}
