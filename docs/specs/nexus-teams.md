# Nexus Teams

## Problem Statement

Nexus could run one agent, or several agents in parallel worktrees on a fixed task graph. Real work rarely looks like that. A goal such as "add a health endpoint with tests" or "pick a sync engine for our app" needs several perspectives (planning, building, verifying, challenging, writing). It needs coordination as the plan changes, a way to resolve disagreement, and a person who can step in without micromanaging.

Developers already know how to run that kind of collaboration: a team in a chat workspace like Microsoft Teams, with channels, mentions, direct messages, threads, a task list, and sign-off. Nexus had no equivalent for bots. The user could not assemble a group of bots with different roles, give the group a goal, and let it organize itself toward that goal while staying in control.

## Solution

Nexus Teams is a collaboration layer in which **bots talk to each other** in a Teams-like workspace and work toward a goal the user sets.

- The user builds **bots** from ten archetypes or from their own profiles, and combines them into **teams** (saved, templated, or ad hoc).
- A **mission** gives a team a goal. Each **round**, a **floor policy** picks who speaks. Each bot's **brain** reads the conversation, board, and goal, and replies in prose plus structured **actions** (tasks, DMs, channels, handoffs, questions, proposals, votes).
- The **mission engine** checks every action against permissions and domain rules, then applies it. It enforces budgets, detects stalls, and settles completion through **governance**: the lead proposes, approvers vote under an approval rule, and the human signs off if required.
- Everything is an **event**. Missions persist to SQLite, can be replayed and exported, and can be **resumed** with a fresh budget and feedback.
- The user steers through an **operator console**: an interactive CLI or a full-screen TUI. They can chat, DM bots, answer questions, approve or reject, pause, and cancel.

## User Stories

1. As a developer, I want to give a goal to a ready-made team (software, research, content, incident, debate) with one command, so I can see bots collaborate without configuring anything.
2. As a developer, I want to create bots with roles, expertise, instructions, tools, models, and permissions, so each bot behaves like the teammate I need.
3. As a team designer, I want to save teams with a lead, approvers, channels, a floor policy, and a budget, so I can rerun the same team on new goals.
4. As an operator, I want to watch the conversation live in channels and threads, so I understand why the team is doing what it does.
5. As an operator, I want to mention or DM a bot, answer its questions, and pause or cancel the mission, so I stay in control without doing the work myself.
6. As a reviewer, I want completion to require approval from designated bots (all, a majority, or any) and optionally from me, so "done" means something.
7. As a security-conscious user, I want every bot action and tool call to go through permissions, and bot code changes to stay in isolated worktrees until I merge them explicitly.
8. As a cost-conscious user, I want rounds, turns, messages, and tokens to be bounded, and stalled teams to stop, so a mission never runs away.
9. As a user returning later, I want to list past missions, read transcripts, inspect the board, and resume a mission with feedback.
10. As a developer without API keys, I want missions to run offline and deterministically, so demos and tests work anywhere.

## Implementation Decisions

### Crate and dependency boundaries

`nexus-teams` is a new crate. It depends on `nexus-permissions`, `nexus-storage`, `nexus-context` (token estimates), `nexus-models`, `nexus-runtime`, `nexus-tools`, and `nexus-config` for the model-backed brain. It does not depend on the CLI, the TUI, or Git. Interfaces inject provider factories, executor factories, and workspace allocation, so provider HTTP and Git details stay outside the engine.

| Module | Responsibility |
| --- | --- |
| `roster` | `BotProfile`, the ten archetypes, handle rules, workspace mode |
| `team` | `TeamSpec`, `FloorPolicyKind`, `ApprovalRule`, `Budget`, member specs (`ada:engineer`) |
| `conversation` | channels, direct channels (`dm:a+b`), messages, kinds, threads, mentions |
| `board` | `TaskBoard`, task ids (`T3`), statuses, dependency validation and readiness |
| `protocol` | `BotAction`, `parse_reply`, `protocol_guide`, usage accounting |
| `mission` | `TeamEvent`, `MissionState::apply` / `replay`, status, proposals, reports |
| `floor` | floor policies and expertise scoring |
| `brain` | `BotBrain`, `SimulatedBrain`, `ScriptedBrain`, `PacedBrain`, `BrainRouter` |
| `context` | per-turn prompt rendering within a token budget |
| `engine` | `MissionEngine`, `MissionControl`, sinks, governance, budgets, resume |
| `runtime_brain` | `RuntimeBrain`: bots backed by `AgentRuntime`, with permissioned tools |
| `library`, `templates` | `.nexus/bots` and `.nexus/teams` files, overrides, built-in templates |
| `persistence` | SQLite sink, mission listing, prefix resolution, loading |
| `operator` | one shared parser for operator commands (CLI and TUI) |
| `transcript` | Markdown transcripts and reports |

### Mission lifecycle

```text
          ┌──────────── pause ───────────┐
          ▼                              │
 start ─▶ active ◀── resume ── paused ◀──┘
          │  ▲
 ask_human│  │answer / timeout
          ▼  │
     awaiting human
          │
 propose  ▼
     reviewing ──(approvers vote; rule decides)──▶ rejected → active
          │
          └──▶ accepted ──(human approver?)──▶ awaiting human ──▶ completed / active

 terminal: completed · stalled · out of budget · failed · cancelled   (resume reopens)
```

### Round loop

1. Check cancellation, drain operator input, and wait while paused.
2. If the round budget is spent, finish as out of budget.
3. Pick speakers. While reviewing, these are the approvers who still owe a vote; otherwise the floor policy chooses.
4. Run turns sequentially, or in parallel for `broadcast` or `parallel_turns`. Each turn gets a snapshot of the state, a token-budgeted prompt, a timeout, and a child cancellation token.
5. Authorize and apply each action, emitting events.
6. Settle any proposal, check token, turn, and message budgets, and track stalls (nudge the lead one round before stopping).

### Floor policies

- **lead-directed** (default): the lead plans when the board is empty. Then mentioned bots and assignees with actionable tasks speak, and the lead returns when the team is idle or every task is done.
- **mention-driven**: whoever was mentioned, then assignees, with the lead as a fallback.
- **round-robin**: everyone in roster order.
- **broadcast**: everyone at once, in parallel.
- **expertise**: mentions and assignees first, then up to two bots whose expertise overlaps most with recent conversation.

### Action protocol

See [ADR 0002](../adr/0002-bot-action-protocol.md). Bots reply in prose with one optional `nexus-actions` JSON block. Actions are post, direct_message, create_task, assign_task, update_task, create_channel, note, ask_human, handoff, propose_completion, vote, and pass.

### Governance

- Only the lead may propose completion by default (a permission rule).
- Bot approvers vote. `ApprovalRule::decide(approvals, rejections, voters)` settles the proposal as early as possible: `all` rejects on the first rejection; `majority` needs more than half; `any` needs one approval.
- Rejections carry every objection back to the team, and work resumes.
- If `human` is an approver, an accepted bot vote waits for `/approve` or `/reject`. Missions that require a human refuse to start without an operator.
- With no approvers, the lead's proposal settles the mission.

### Safety and bounds

- Every action is permissioned ([ADR 0003](../adr/0003-permissioned-team-actions.md)). Domain rules gate task updates (assignee, creator, or lead only; dependencies done first), channel posting (membership), and votes (approvers only, one each).
- Model-backed bots use workspace tools only through `AuthorizedToolExecutor` (`filesystem.read` allowed, `shell.execute` asks).
- Bots with `workspace = "worktree"` get their own Git worktree, allocated lazily per mission and reused on resume. Nothing is merged automatically; the CLI prints the `nexus worktree review` commands after a mission.
- Budgets cover rounds, turns, messages, and tokens. Other bounds: stall rounds, a per-turn timeout, a human response timeout, and cancellation through `MissionControl` or Ctrl+C.

### Persistence and resume

See [ADR 0001](../adr/0001-event-sourced-missions.md). Missions are event streams `mission-<uuid>` in `.nexus/nexus.db`, addressable by a unique prefix. `resume` grants extra rounds, turns, and tokens on top of what was spent and posts the operator's note as `@human`. A completed mission reopens only with a note; without one, a resumed team would just re-approve finished work. The simulated lead turns such feedback into a follow-up task routed by mention or expertise.

### Surfaces

- **CLI**: `nexus bots archetypes|list|show|new` and `nexus team templates|list|show|new|run|resume|missions|transcript|board|report`. `team run -i` attaches the operator console on stdin.
- **TUI**: `nexus-tui` renders channels with unread counts, the roster, threaded messages, the board, decisions, and activity. It accepts the same operator grammar and supports `--template`, `--replay`, and `--resume`.
- Both surfaces assemble teams through `TeamLibrary::assemble` with `TeamOverrides`, so flags behave identically.

## Testing Decisions

- The engine is tested end to end with offline brains: `SimulatedBrain` for realistic flows and `ScriptedBrain` for exact scenarios. That includes completion, rejections under each approval rule, stalls, budgets, timeouts, cancellation, human questions and approvals, DMs, threads, handoffs, dependency gates, permissions, and resume.
- Replay fidelity is asserted: folding the persisted events gives the engine's final state.
- Interfaces test flag parsing and assembly. The TUI renders real missions into a `ratatui` `TestBackend`.
- Human-in-the-loop tests send operator input only after the engine is waiting, and run under a hard timeout so a regression cannot hang the suite.

## Out of Scope

- Automatic merging of bot changes into the human checkout (forbidden by project rules).
- Hosted multi-user workspaces and authentication. Nexus Teams is local-first.
- Training or fine-tuning models.

## Further Notes

Directions for making teams more capable, roughly in priority order:

1. **Memory across missions.** Per-bot and per-team knowledge (decisions, conventions, lessons from rejected proposals), stored as events and summarized into prompts, so a team gets better at a codebase over time.
2. **Teams of teams.** A lead can delegate a sub-goal to another team as a child mission, so missions form a tree, with results and budgets rolling up and governance at each level.
3. **Streaming turns.** Stream model output into the console as bots "type", with cancellation mid-turn.
4. **Richer tools per bot.** Grant MCP servers and skills per bot. Run worktree tool calls in containers. Add per-bot tool budgets.
5. **Cost-aware budgets.** Price tokens per provider and model, show spend live, and stop on a currency budget.
6. **Triggers and schedules.** Start missions from GitHub issues (`ready-for-agent`), CI failures, or a schedule, and report back to the issue.
7. **Meetings.** Time-boxed "stand-ups" and "retros" as special rounds whose output updates the board and team memory.
8. **Bridges and dashboards.** A web view of live missions, and bridges that mirror channels into Slack or Microsoft Teams so people can join the conversation where they already are.
9. **Learning floor policies.** Use member stats (denials, failures, approvals) to adjust who gets the floor and who reviews.
