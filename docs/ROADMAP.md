# Nexus Roadmap

Nexus is developed as composable runtime primitives. Each phase leaves reusable contracts for the phases that follow.

## Phase 1 — Execution Core ✅

- [x] Model contracts, streaming, and real OpenAI-compatible provider
- [x] Tool registry, filesystem and structured shell tools
- [x] Explicit allow/ask/deny/sandbox permissions and CLI approvals
- [x] Bounded model-driven tool loop and cancellation
- [x] Structured audit events, SQLite persistence, and replay
- [x] Git worktree lifecycle

**Exit criteria: complete.**

## Phase 2 — Context + Sessions ✅

- [x] Layered user/project/environment configuration
- [x] Repository discovery, budgets, and token inspection
- [x] Hierarchical `AGENTS.md` and instruction composition
- [x] Git-aware context prioritization
- [x] Deterministic local code indexing/search
- [x] Durable sessions and ordered replay

**Exit criteria: complete.**

## Phase 3 — Parallel Agents 🟡

### Implemented
- [x] One isolated worktree per agent
- [x] Partial allocation rollback and deterministic result ordering
- [x] Bounded concurrent real Nexus runs
- [x] Task graphs with dependency validation and execution layers
- [x] Supervisor/worker/reviewer orchestration plans
- [x] Explicit workspace cleanup policies: keep, remove-clean, remove-always
- [x] Workspace backend abstraction for future container/remote implementations
- [x] Explicit human review/merge command (`nexus worktree review`, `nexus worktree merge --approve`)

### Next
- [ ] Shared run coordinator with cancellation fan-out
- [ ] Execute supervisor/reviewer plans as first-class runtime workflows
- [ ] Result aggregation strategies and structured handoffs
- [ ] Container workspace backend

## Phase 4 — Extensibility + Interfaces 🟡

### Implemented
- [x] MCP stdio JSON-RPC client
- [x] `tools/list` and `tools/call`
- [x] MCP tool adapters that implement Nexus `Tool`
- [x] Project-local skills and agent templates
- [x] Hook configuration and lifecycle model
- [x] Teams operator console TUI: live missions, chat, approvals, replay, and resume

### Next
- [ ] Register MCP adapters from configured servers into runtime tool registries
- [ ] Permission-controlled hook execution through structured shell requests
- [ ] Plugin manifests, discovery, compatibility checks, and capability boundaries
- [ ] Public SDK seams
- [ ] TUI views for single-agent runs, context, and workspaces
- [ ] Tauri desktop shell
- [ ] Remote workers and workspace backends

## Phase 5 — Nexus Teams 🟡

Bots that collaborate like people in Microsoft Teams: the user builds groups of bots, gives them a goal, and they plan, divide work, talk, and vote until the goal is met. The design is in [`specs/nexus-teams.md`](specs/nexus-teams.md).

### Implemented
- [x] `nexus-teams` crate with a dependency-light domain model (roster, channels, task board, missions)
- [x] Ten bot archetypes, project bots (`.nexus/bots`), saved teams (`.nexus/teams`), and five team templates
- [x] Channels, direct messages (bot↔bot and bot↔human), threads, mentions, and handoffs
- [x] Shared task board with assignees, cycle-checked dependencies, and gated status changes
- [x] Floor policies: lead-directed, round-robin, mention-driven, broadcast, and expertise routing; parallel turns
- [x] Structured bot action protocol (`nexus-actions` JSON) with tolerant parsing
- [x] Every bot action authorized through `nexus-permissions` (`team.*` actions, per-bot overrides)
- [x] Completion governance: proposals, votes, `all` / `majority` / `any` rules, and human sign-off
- [x] Budgets (rounds, turns, messages, tokens), stall detection, turn and human timeouts, pause/resume, cancellation
- [x] Event-sourced missions persisted to SQLite: list, replay, transcript (chat/Markdown/JSON), board, report
- [x] Resume with a fresh budget and operator feedback; completed missions reopen only with follow-up work
- [x] Model-backed bots through `AgentRuntime` with per-bot providers, models, permissioned tools, and Git worktrees
- [x] CLI surfaces: `nexus bots …`, `nexus team …` with an interactive operator console
- [x] TUI surface: Teams-style console with channels, roster, board, votes, and activity

### Next
- [ ] Bot memory that carries across missions (per-bot and per-team knowledge)
- [ ] Teams that delegate sub-goals to other teams (mission trees)
- [ ] Streaming bot replies into the console as they are generated
- [ ] Per-mission cost accounting and budgets in currency, not just tokens
- [ ] Scheduled and recurring missions; triggers from issues, CI, or webhooks
- [ ] MCP tools and skills granted per bot
- [ ] Web dashboard and chat bridges (Slack, Microsoft Teams)

## Immediate priority

Finish the remaining Phase 3 orchestration vertical slice first, then proceed through Phase 4 interfaces:

```text
Task graph
   ↓
Worker worktrees
   ↓
Supervisor aggregation
   ↓
Reviewer verification
   ↓
Human review / explicit merge
```

For Nexus Teams, the next slice is cross-mission memory, then delegation between teams. Both build on the existing event log, so they can be added without changing the mission contract.

Nexus must continue to preserve explicit permissions, isolated workspaces, observable execution, and no silent autonomous merges.
