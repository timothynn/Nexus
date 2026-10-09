# Nexus Context

Nexus is a local-first, model-agnostic harness for running AI agents and teams of collaborating bots. Its pieces (models, tools, permissions, context, sessions, workspaces, skills, hooks, plugins, and teams) are separate primitives behind explicit contracts.

## Constraints

- `nexus-core` stays dependency-light. Provider HTTP, Git details, and UI never leak into core contracts.
- Every tool call and every bot action passes through `nexus-permissions`.
- Agent and bot changes are never merged automatically into the human checkout. Merging requires an explicit, human-approved step.
- Auditability and cancellation are part of every execution contract.
- New integrations are additive and isolated in their own crates or adapters.

## Glossary: execution

- **Agent run**: one bounded model-driven loop (`AgentRuntime`) that may call tools.
- **Tool**: a named capability (`filesystem.read`, `shell.execute`, MCP tools) executed through a permissioned executor.
- **Permission policy**: maps an action string to allow, ask, deny, or sandbox. An **approver** resolves `ask`.
- **Worktree**: an isolated Git checkout for an agent or bot. A human reviews it and merges it with `--approve`.
- **Session / event stream**: an append-only, ordered list of events in SQLite. State is rebuilt by replaying it.

## Glossary: Nexus Teams

- **Bot**: a team member persona with a handle (`@ada`), role, expertise, instructions, tools, optional provider/model, workspace mode, and permission overrides.
- **Archetype**: a built-in bot template (lead, architect, engineer, researcher, reviewer, tester, writer, critic, designer, analyst). A project bot file with the same handle overrides it.
- **Team**: a lead, members, channels, approvers, an approval rule, a floor policy, a budget, and whether turns run in parallel.
- **Template**: a built-in team blueprint (software, research, content, incident, debate).
- **Lead**: the member who plans, assigns work, and proposes completion. By default the lead is the only member allowed to propose.
- **Operator / `@human`**: the person running the mission. They can chat, DM bots, answer questions, approve or reject completion, pause, resume, and cancel.
- **Mission**: one run of a team toward a **goal** (a statement plus optional success criteria). It has a status (active, paused, awaiting human, reviewing, or a terminal status: completed, stalled, out of budget, failed, cancelled).
- **Round**: one pass in which the floor policy selects speakers and each one takes a turn.
- **Turn**: one bot's chance to act, producing a reply (prose plus optional actions) from its **brain**.
- **Brain**: what thinks for a bot (`BotBrain`): `SimulatedBrain` (offline and deterministic), `RuntimeBrain` (model-backed), `ScriptedBrain` (tests), or `PacedBrain` (adds a delay so a person can follow along).
- **Floor policy**: chooses who speaks each round: lead-directed, round-robin, mention-driven, broadcast, or expertise.
- **Channel**: a named conversation (`#general`, topic channels). A **direct channel** (`dm:a+b`) is private to two participants. A **thread** groups replies under a root message.
- **Mention**: `@handle` (or `@all`) in a message. It routes attention and floor selection.
- **Task board**: the team's shared tasks (`T1`, `T2`, …) with assignee, status, dependencies, creator, and note.
- **Action**: a structured step a bot takes (post, direct message, create/assign/update task, create channel, note, ask human, handoff, propose completion, vote, pass), sent as a `nexus-actions` JSON block.
- **Proposal**: the lead's claim that the goal is met. **Approvers** vote on it, and the **approval rule** (`all`, `majority`, `any`) settles it, followed by `@human` sign-off when the human is an approver.
- **Budget**: limits on rounds, turns, messages, and tokens, plus stall rounds and turn and human timeouts.
- **Stall**: rounds without progress. The engine nudges the lead first, then stops the mission as stalled.
- **Resume**: continuing a persisted mission with a fresh budget and optional operator note. A completed mission reopens only with a note.

## Decisions

- [ADR 0001: Event-sourced missions](docs/adr/0001-event-sourced-missions.md)
- [ADR 0002: A text-embedded action protocol for bots](docs/adr/0002-bot-action-protocol.md)
- [ADR 0003: Bot actions are permissioned like tool calls](docs/adr/0003-permissioned-team-actions.md)
