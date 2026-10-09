# ADR 0001: Event-sourced missions

- **Status:** Accepted
- **Area:** `nexus-teams`, `nexus-storage`, CLI, TUI

## Context

A team mission is a long, multi-party conversation with side effects: tasks change hands, channels open, votes are cast, budgets run down, and a human may step in at any point. Several surfaces need the same picture of a mission:

- the engine, to pick speakers and enforce rules;
- the CLI chat feed and the TUI, to render it live;
- transcripts, reports, and the task board after the fact;
- `resume`, to continue a mission days later.

Auditability is part of the Nexus execution contract, so the record of what happened has to be complete and trustworthy, not a best-effort log.

## Options considered

1. **Mutable state plus a separate log.** The engine mutates a `MissionState` and also writes log lines. This is simple, but the log and the state can drift, and replay is only approximate.
2. **Snapshot persistence.** Serialize the whole state after each round. Resume is easy, but history and per-event audit are lost, and live views still need a separate feed.
3. **Event sourcing.** Every change is a `TeamEvent`. State is a pure fold (`MissionState::apply`) over events, and the same events feed the UI, persistence, and replay.

## Decision

Missions are event-sourced.

- The engine changes state only by emitting `TeamEvent`s: it applies each one to its own `MissionState` and fans it out to every `TeamEventSink`.
- `MissionState::replay(events)` rebuilds a mission exactly. Tests assert that replayed state equals the engine's final state.
- `SqliteTeamSink` appends each event to a stream named after the mission id (`mission-<uuid>`) in `.nexus/nexus.db`. A persistence failure never interrupts a mission; it is collected and reported.
- Live surfaces (CLI printer, TUI) consume the same events through `ChannelEventSink`.
- `MissionEngine::resume` loads the folded state and appends a `MissionResumed` event (carrying the new budget) to the same stream, so one id holds the whole history across sessions.

## Consequences

- Transcripts (chat, Markdown, JSON), reports, the board view, and the TUI replay all derive from one source of truth.
- Adding a feature means adding an event variant and an `apply` arm, and keeping replay exact. Old streams stay readable because new variants are additive and use serde tags.
- Events carry full payloads (messages, tasks, the team spec at start), so streams grow with the conversation. That is acceptable for local SQLite and keeps replay self-contained.
- Engine-internal state (the floor policy cursor, the stall counter) is not persisted. Resume starts those fresh, and that is intended.
