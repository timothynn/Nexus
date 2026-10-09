//! Durable missions: persist events to `SQLite` and rebuild missions by replay.

use std::sync::{Arc, Mutex};

use nexus_storage::{SqliteStore, StreamSummary};

use crate::{MissionId, MissionState, MissionStatus, TeamError, TeamEvent, TeamEventSink};

/// Prefix shared by every mission event stream.
pub const MISSION_PREFIX: &str = "mission-";

/// Appends every mission event to a `SQLite` event stream named after the mission id.
///
/// Persistence failures never interrupt a running mission; they are collected
/// and can be inspected with [`SqliteTeamSink::errors`].
pub struct SqliteTeamSink {
    store: Arc<SqliteStore>,
    errors: Mutex<Vec<String>>,
}

impl SqliteTeamSink {
    #[must_use]
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self {
            store,
            errors: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|errors| errors.clone())
            .unwrap_or_default()
    }
}

impl TeamEventSink for SqliteTeamSink {
    fn record(&self, mission: &MissionId, event: &TeamEvent) {
        let result = serde_json::to_value(event)
            .map_err(|error| error.to_string())
            .and_then(|payload| {
                self.store
                    .append_event(&mission.0, event.kind(), &payload)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            if let Ok(mut errors) = self.errors.lock() {
                errors.push(error);
            }
        }
    }
}

/// Loads every persisted event of a mission, in order.
pub fn load_events(store: &SqliteStore, mission: &str) -> Result<Vec<TeamEvent>, TeamError> {
    store
        .replay(mission)
        .map_err(|error| TeamError::Config(error.to_string()))?
        .into_iter()
        .map(|stored| {
            serde_json::from_value(stored.payload).map_err(|error| {
                TeamError::Config(format!(
                    "event {} of {mission} is unreadable: {error}",
                    stored.sequence
                ))
            })
        })
        .collect()
}

/// Rebuilds a mission's state from its persisted events.
pub fn load_mission(store: &SqliteStore, mission: &str) -> Result<MissionState, TeamError> {
    let events = load_events(store, mission)?;
    MissionState::replay(&events).ok_or_else(|| TeamError::NotFound(format!("mission `{mission}`")))
}

/// Index row for a persisted mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionSummary {
    pub id: String,
    pub team: String,
    pub goal: String,
    pub status: MissionStatus,
    pub events: u64,
    pub last_at_ms: i64,
}

/// Lists persisted missions, newest first. Unreadable streams are skipped.
pub fn list_missions(store: &SqliteStore) -> Result<Vec<MissionSummary>, TeamError> {
    let streams = store
        .list_streams(MISSION_PREFIX)
        .map_err(|error| TeamError::Config(error.to_string()))?;
    Ok(streams
        .into_iter()
        .filter_map(|stream: StreamSummary| {
            let state = load_mission(store, &stream.id).ok()?;
            Some(MissionSummary {
                id: stream.id,
                team: state.team.name,
                goal: state.goal.statement,
                status: state.status,
                events: stream.events,
                last_at_ms: stream.last_at_ms,
            })
        })
        .collect())
}

/// Resolves a unique mission id from a prefix (like Git short hashes).
pub fn resolve_mission_id(store: &SqliteStore, prefix: &str) -> Result<String, TeamError> {
    let wanted = if prefix.starts_with(MISSION_PREFIX) {
        prefix.to_owned()
    } else {
        format!("{MISSION_PREFIX}{prefix}")
    };
    let matches = store
        .list_streams(&wanted)
        .map_err(|error| TeamError::Config(error.to_string()))?;
    match matches.as_slice() {
        [only] => Ok(only.id.clone()),
        [] => Err(TeamError::NotFound(format!("mission `{prefix}`"))),
        many => Err(TeamError::Config(format!(
            "mission prefix `{prefix}` is ambiguous ({} matches)",
            many.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexus_storage::SqliteStore;

    use super::{SqliteTeamSink, list_missions, load_mission, resolve_mission_id};
    use crate::{
        BotProfile, BrainRouter, Goal, MissionEngine, MissionStatus, SimulatedBrain, TeamSpec,
    };

    #[tokio::test]
    async fn missions_persist_and_replay() {
        let store = Arc::new(SqliteStore::open(":memory:").expect("store"));
        let sink = Arc::new(SqliteTeamSink::new(Arc::clone(&store)));
        let team = TeamSpec::new(
            "persist",
            vec![
                BotProfile::from_archetype("lead", "lead").expect("lead"),
                BotProfile::from_archetype("ada", "engineer").expect("eng"),
            ],
        );
        let engine = MissionEngine::new(
            team,
            Goal::new("Persist me"),
            BrainRouter::new(Arc::new(SimulatedBrain)),
        )
        .expect("engine")
        .with_sink(sink.clone());
        let id = engine.mission_id().0.clone();
        let report = engine.run().await.expect("report");
        assert!(sink.errors().is_empty());

        let state = load_mission(&store, &id).expect("load");
        assert_eq!(state.status, report.status);
        assert_eq!(state.status, MissionStatus::Completed);
        assert_eq!(state.message_count(), report.messages);

        let missions = list_missions(&store).expect("list");
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].goal, "Persist me");

        let short = &id["mission-".len().."mission-".len() + 6];
        assert_eq!(resolve_mission_id(&store, short).expect("resolve"), id);
        assert!(resolve_mission_id(&store, "zzzz").is_err());
    }
}
