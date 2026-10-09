//! The team task board (Planner-style) with dependency-aware readiness.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::TeamError;

/// Task identifier rendered as `T<n>`. Accepts `3`, `"3"`, or `"T3"` when deserialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskId(pub u32);

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "T{}", self.0)
    }
}

impl FromStr for TaskId {
    type Err = TeamError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        let digits = trimmed
            .strip_prefix('T')
            .or_else(|| trimmed.strip_prefix('t'))
            .unwrap_or(trimmed);
        digits
            .parse()
            .map(TaskId)
            .map_err(|_| TeamError::InvalidTaskId(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u32),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Number(number) => Ok(TaskId(number)),
            Raw::Text(text) => text.parse().map_err(de::Error::custom),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Review,
    Done,
    Cancelled,
}

impl TaskStatus {
    #[must_use]
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Done | Self::Cancelled)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in progress",
            Self::Blocked => "blocked",
            Self::Review => "review",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardTask {
    pub id: TaskId,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub assignee: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    pub created_by: String,
    /// Latest progress or result note.
    #[serde(default)]
    pub note: Option<String>,
}

/// Board mutations; the engine validates them before turning them into events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBoard {
    tasks: BTreeMap<TaskId, BoardTask>,
}

impl TaskBoard {
    #[must_use]
    pub fn next_id(&self) -> TaskId {
        TaskId(self.tasks.keys().next_back().map_or(1, |id| id.0 + 1))
    }

    #[must_use]
    pub fn get(&self, id: TaskId) -> Option<&BoardTask> {
        self.tasks.get(&id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Inserts or replaces a task snapshot (used when applying events).
    pub fn upsert(&mut self, task: BoardTask) {
        self.tasks.insert(task.id, task);
    }

    /// Validates dependencies for a new or changed task: they must exist and not form a cycle.
    pub fn validate_dependencies(
        &self,
        id: TaskId,
        depends_on: &[TaskId],
    ) -> Result<(), TeamError> {
        for dependency in depends_on {
            if *dependency == id {
                return Err(TeamError::TaskCycle(id));
            }
            if !self.tasks.contains_key(dependency) {
                return Err(TeamError::UnknownTask(*dependency));
            }
            if self.reaches(*dependency, id) {
                return Err(TeamError::TaskCycle(id));
            }
        }
        Ok(())
    }

    fn reaches(&self, from: TaskId, target: TaskId) -> bool {
        let mut stack = vec![from];
        let mut seen = Vec::new();
        while let Some(current) = stack.pop() {
            if current == target {
                return true;
            }
            if seen.contains(&current) {
                continue;
            }
            seen.push(current);
            if let Some(task) = self.tasks.get(&current) {
                stack.extend(task.depends_on.iter().copied());
            }
        }
        false
    }

    /// A task is ready when it is open and all its dependencies are done.
    #[must_use]
    pub fn is_ready(&self, task: &BoardTask) -> bool {
        task.status.is_open()
            && task.depends_on.iter().all(|dependency| {
                self.tasks
                    .get(dependency)
                    .is_some_and(|task| task.status == TaskStatus::Done)
            })
    }

    /// Open, ready tasks assigned to a member, in id order.
    #[must_use]
    pub fn actionable_for(&self, handle: &str) -> Vec<&BoardTask> {
        self.tasks
            .values()
            .filter(|task| task.assignee.as_deref() == Some(handle))
            .filter(|task| matches!(task.status, TaskStatus::Todo | TaskStatus::InProgress))
            .filter(|task| self.is_ready(task))
            .collect()
    }

    #[must_use]
    pub fn open_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| task.status.is_open())
            .count()
    }

    #[must_use]
    pub fn count(&self, status: TaskStatus) -> usize {
        self.tasks
            .values()
            .filter(|task| task.status == status)
            .count()
    }

    /// Whether every task is closed and at least one is done.
    #[must_use]
    pub fn all_done(&self) -> bool {
        !self.tasks.is_empty() && self.open_count() == 0 && self.count(TaskStatus::Done) > 0
    }

    /// Compact board rendering for prompts and terminals.
    #[must_use]
    pub fn render(&self) -> String {
        if self.tasks.is_empty() {
            return "(no tasks yet)".to_owned();
        }
        self.tasks
            .values()
            .map(|task| {
                let assignee = task
                    .assignee
                    .as_deref()
                    .map_or_else(|| "unassigned".to_owned(), |handle| format!("@{handle}"));
                let deps = if task.depends_on.is_empty() {
                    String::new()
                } else {
                    format!(
                        " after {}",
                        task.depends_on
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                };
                let note = task
                    .note
                    .as_deref()
                    .map(|note| format!(" — {note}"))
                    .unwrap_or_default();
                format!(
                    "{} [{}] {} ({assignee}{deps}){note}",
                    task.id,
                    task.status.label(),
                    task.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{BoardTask, TaskBoard, TaskId, TaskStatus};

    fn task(id: u32, status: TaskStatus, depends_on: &[u32]) -> BoardTask {
        BoardTask {
            id: TaskId(id),
            title: format!("task {id}"),
            description: String::new(),
            assignee: Some("coder".to_owned()),
            status,
            depends_on: depends_on.iter().copied().map(TaskId).collect(),
            created_by: "lead".to_owned(),
            note: None,
        }
    }

    #[test]
    fn task_ids_parse_flexibly() {
        assert_eq!("T3".parse::<TaskId>().expect("parses"), TaskId(3));
        assert_eq!("7".parse::<TaskId>().expect("parses"), TaskId(7));
        assert!("x".parse::<TaskId>().is_err());
        let from_json: Vec<TaskId> = serde_json::from_str(r#"[1, "T2", "3"]"#).expect("json");
        assert_eq!(from_json, vec![TaskId(1), TaskId(2), TaskId(3)]);
    }

    #[test]
    fn readiness_waits_for_dependencies() {
        let mut board = TaskBoard::default();
        board.upsert(task(1, TaskStatus::InProgress, &[]));
        board.upsert(task(2, TaskStatus::Todo, &[1]));
        assert_eq!(board.actionable_for("coder").len(), 1);
        board.upsert(task(1, TaskStatus::Done, &[]));
        assert_eq!(board.actionable_for("coder")[0].id, TaskId(2));
    }

    #[test]
    fn cycles_and_unknown_dependencies_are_rejected() {
        let mut board = TaskBoard::default();
        board.upsert(task(1, TaskStatus::Todo, &[]));
        board.upsert(task(2, TaskStatus::Todo, &[1]));
        assert!(
            board
                .validate_dependencies(TaskId(1), &[TaskId(2)])
                .is_err()
        );
        assert!(
            board
                .validate_dependencies(TaskId(3), &[TaskId(9)])
                .is_err()
        );
        assert!(
            board
                .validate_dependencies(TaskId(3), &[TaskId(3)])
                .is_err()
        );
        assert!(
            board
                .validate_dependencies(TaskId(3), &[TaskId(1), TaskId(2)])
                .is_ok()
        );
    }

    #[test]
    fn completion_requires_closed_board_with_done_work() {
        let mut board = TaskBoard::default();
        assert!(!board.all_done());
        board.upsert(task(1, TaskStatus::Cancelled, &[]));
        assert!(!board.all_done());
        board.upsert(task(2, TaskStatus::Done, &[]));
        assert!(board.all_done());
        assert_eq!(board.next_id(), TaskId(3));
    }

    #[test]
    fn rendering_lists_status_and_assignee() {
        let mut board = TaskBoard::default();
        board.upsert(task(1, TaskStatus::Todo, &[]));
        assert_eq!(board.render(), "T1 [todo] task 1 (@coder)");
    }
}
