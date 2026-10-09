# ADR 0003: Bot actions are permissioned like tool calls

- **Status:** Accepted
- **Area:** `nexus-teams` (`engine.rs`), `nexus-permissions`

## Context

Nexus requires every tool execution to pass through permissions. Team bots act on shared state that others rely on: they post to channels, open channels, reassign tasks, ask the human for input, and propose that the goal is met. Without checks, a confused or adversarial bot could approve its own work, spam the human, or take over the board.

## Decision

Every `BotAction` maps to a `team.*` action string and is authorized with `nexus-permissions` before the engine applies it:

| Action | Permission |
| --- | --- |
| post | `team.message.post` |
| direct_message | `team.message.direct` |
| create_task / assign_task / update_task | `team.task.create` / `team.task.assign` / `team.task.update` |
| create_channel | `team.channel.create` |
| note | `team.note` |
| ask_human | `team.human.ask` |
| handoff | `team.handoff` |
| propose_completion | `team.goal.propose` |
| vote | `team.goal.vote` |
| pass | `team.pass` |

- Each bot gets a `RuleBasedPolicy`: allow by default; `team.goal.propose` allowed only for the lead; then the bot profile's own `permissions` overrides (exact actions or wildcards such as `team.task.*`).
- An optional operator-wide policy is checked first. Both must allow, so the stricter decision wins. `ask` decisions go to the configured `PermissionApprover`.
- A denial emits `ActionDenied` (counted in member stats) and a private engine note to the bot, so it can adapt on its next turn.
- Domain rules apply on top of permissions. For example, only the assignee, creator, or lead may update a task; work cannot start before its dependencies are done; and votes count only from approvers while a proposal is open.
- Human approval of completion is governance (the `human` approver plus the approval rule), not a permission prompt, so it works the same in the CLI console and the TUI.

## Consequences

- Teams can be locked down per bot in TOML (for example, a critic that may not create tasks) without code changes.
- Permission decisions and denials are part of the event log and therefore auditable and replayable.
- Workspace tools used inside a model-backed turn are still governed separately by the tool executor's policy (`filesystem.read` allowed, `shell.execute` asks by default).
