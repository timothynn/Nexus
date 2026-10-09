<div align="center">

# ◈ Nexus

### The programmable AI harness for developers, agents, and autonomous workflows.

**Local-first · Model-agnostic · Tool-native · Workspace-isolated · Observable by design**

[**Quick Start**](#-quick-start) · [**Nexus Teams**](#-nexus-teams--bots-that-work-together) · [**Working Now**](#-whats-working-now) · [**Architecture**](#-architecture) · [**Roadmap**](#-roadmap)

</div>

---

> **Nexus is not another AI chat wrapper.** It is a configurable runtime where models, agents, tools, permissions, context, sessions, workspaces, skills, hooks, plugins, and integrations are composable primitives.

# 🚀 Quick Start

```bash
git clone https://github.com/timothynn/Nexus.git
cd Nexus
cargo build --workspace
cargo run -p nexus-cli -- run "inspect this repository"
```

Put a team of bots to work on a goal (runs offline with simulated bots by default):

```bash
cargo run -p nexus-cli -- team run --template software --goal "Add a /healthz endpoint with tests"
```

Launch the interactive Teams operator console:

```bash
cargo run -p nexus-tui -- --template research -g "Pick a sync engine for a local-first app"
```

## Multi-agent orchestration

```bash
nexus agents graph \
  research \
  implement:research \
  tests:implement \
  review:tests \
  --concurrency 2 --tools
```

# 🤝 Nexus Teams — bots that work together

Think Microsoft Teams, except the members are bots. You assemble a team, give it a goal, and the bots plan the work, split it into tasks, talk in channels and direct messages, hand work to each other, and vote on whether the goal is met. You watch, chat with them, answer their questions, and approve or reject the result.

```text
        you (@human): goal · chat · /dm · /answer · /approve · /reject · /pause · /cancel
                                          │
                                          ▼
 team ─▶ mission engine ─▶ floor policy picks speakers ─▶ bot brains take turns
              │   ▲                                              │
              │   └──── mentions · tasks · votes · handoffs ◀────┘  (nexus-actions JSON,
              │                                                      checked by permissions)
              ▼
 append-only event log (SQLite) ─▶ replay · transcript · report · board · resume
```

## Quick tour

```bash
nexus team templates                                   # software, research, content, incident, debate
nexus team show research                               # roster, channels, policy, approval, budget
nexus team run --template software --goal "Add a /healthz endpoint with tests"
nexus team run --template incident -i --goal "Checkout latency doubled"   # you sign off with /approve
nexus team missions                                    # persisted missions, newest first
nexus team board 3f2a9c1e                              # task board + working notes
nexus team transcript 3f2a9c1e --format markdown -o mission.md
nexus team resume 3f2a9c1e --rounds 6 --note "Focus on the failing tests"
```

Build your own bots and teams:

```bash
nexus bots archetypes
nexus bots new nova --archetype engineer --expertise rust,tokio --tool filesystem.read --worktree
nexus team new core --template software --member nova --approval majority --channel "ops:Deploys and alerts"
nexus team run core --goal "..." --brain runtime --provider openai-compatible --model <model-id>
```

## What a team has

| Concept | How it works |
| --- | --- |
| **Bots** | Personas with a handle, role, expertise, instructions, tools, model, and permissions. Ten built-in archetypes (lead, architect, engineer, researcher, reviewer, tester, writer, critic, designer, analyst); project bots live in `.nexus/bots/<handle>.toml` and override archetypes of the same name. |
| **Teams** | A lead, members, channels, approvers, an approval rule, a floor policy, and a budget. Saved in `.nexus/teams/<name>.toml`, started from a template, or assembled ad hoc with `--member pm:lead --member ada:engineer`. |
| **Channels, DMs, threads** | `#general` plus topic channels (bots can open more), private `dm:a+b` channels between bots or with you, threaded replies, and `@mentions` that route attention. |
| **Task board** | Tasks with owners, cycle-checked dependencies, and statuses (todo, in progress, blocked, review, done, cancelled). Only the assignee, creator, or lead may update a task, and work starts only once its dependencies are done. |
| **Floor control** | Who speaks each round: `lead-directed` (default), `round-robin`, `mention-driven`, `broadcast`, or `expertise` routing; turns run sequentially or in parallel. |
| **Governance** | The lead proposes completion; approvers vote under the `all`, `majority`, or `any` rule, and `human` can be a required approver. Rejections send the team back to work with the reasons attached. |
| **Bounds** | Round, turn, message, and token budgets; stall detection that nudges the lead before stopping; per-turn and human-response timeouts; pause/resume; Ctrl+C cancellation. |
| **Safety** | Every bot action is checked by `nexus-permissions` as a `team.*` action (only the lead may propose completion by default; profiles can deny more). Model-backed bots call workspace tools through the permissioned executor. |
| **Brains** | `simulated` (deterministic and offline; the default with the mock provider) or `runtime` (model-backed through the Nexus agent runtime). Each bot can use its own provider and model. |
| **Isolation** | Bots with `workspace = "worktree"` get their own Git worktree. Nothing is merged automatically: review with `nexus worktree review <name>` and merge with `nexus worktree merge <name> --approve`. |
| **Durability** | Every mission is an append-only event stream in `.nexus/nexus.db`, and state is rebuilt by replay. Missions can be listed, replayed, exported (chat, Markdown, JSON), reported on, and resumed with a fresh budget and feedback, including reopening a completed mission with follow-up work. |

## Operator console

Run a mission with `-i` (CLI) or in `nexus-tui`, then type:

```text
plain text              post to the current channel (mention bots with @handle)
#channel text           post to another channel
/dm <bot> <text>        private message to one bot
/answer <text>          answer a bot's question
/approve                approve a completion proposal (when @human is an approver)
/reject [reason]        send the team back to work
/pause · /resume        hold or continue the mission
/goal <text>            start a new mission (TUI)
/cancel                 stop the mission
```

The design is described in [`docs/specs/nexus-teams.md`](docs/specs/nexus-teams.md), and the domain vocabulary is defined in [`CONTEXT.md`](CONTEXT.md).

# 🟢 What's Working Now

## Phase 1 — Execution Core ✅

- Provider-neutral models and streaming
- Bounded model-driven tool loop
- Filesystem and structured shell tools
- Permission policies and CLI approval
- Ctrl+C cancellation
- Audit events
- SQLite sessions and replay
- Git worktrees

## Phase 2 — Context + Sessions ✅

- Layered configuration
- Repository discovery and context budgets
- Hierarchical instructions
- Git-aware context
- Code search
- Token estimates

## Phase 3 — Multi-Agent Orchestration ✅

```text
Task Graph → Parallel Workers → Typed Handoffs
                              ↓
                     Supervisor → Reviewer
                              ↓
                     Explicit Human Review
```

Includes bounded concurrency, deterministic task layers, cancellation fan-out, cleanup policies, review candidates, explicit merge boundaries, container workspace foundations, CLI graph execution, and structured run-level events.

## Phase 4 — Extensibility + Isolation 🚧

### Interactive TUI: the Teams operator console

`nexus-tui` is a full-screen, Teams-style console for missions. It shows channels with unread counts, the roster (★ lead, ● speaking), the live conversation with threads, the task board, pending questions and votes, and bot activity. A composer accepts chat and operator commands.

```bash
nexus-tui                                    # demo squad; type /goal <what you want>
nexus-tui --template debate -g "Monorepo or polyrepo?"
nexus-tui core --brain runtime --provider openai-compatible --model <model-id>
nexus-tui --replay 3f2a9c1e                  # read-only replay of a persisted mission
nexus-tui --resume 3f2a9c1e --rounds 6 --note "Tighten the error handling"
```

Keys: `Tab`/`Shift+Tab` focus · `↑↓`/`j k` channels and scrolling · `PgUp`/`PgDn`/`End` · `i` or `/` compose · `p` pause/resume · `a` approve · `x` cancel · `F1` help · `q` quit.

The TUI stays thin. It folds the same `TeamEvent` stream that is persisted and replayed, and it sends operator input through `MissionControl` instead of reimplementing orchestration.

### MCP

```text
MCP Server → MCP Tool Adapter → ToolRegistry → Permissions + Audit
```

### Skills, templates, and hooks

```text
.nexus/skills/<name>/SKILL.md
.nexus/agents/<name>.toml
.nexus/hooks.toml
```

### Container lifecycle

```text
health check → provision → start → execute → stop → remove
```

Safe defaults: network disabled, read-only root, explicit workspace mount, configurable CPU/memory limits, and timeout enforcement.

### Plugin runtime and capability enforcement

```text
plugin.toml → declared capability check → PermissionPolicy
     ↓
entrypoint containment → audited execution
```

# 🧬 Architecture

```text
apps/
├── nexus-cli/                 Scriptable operator surfaces
└── nexus-tui/                 Interactive terminal operator console

crates/
├── nexus-core/                Stable domain contracts
├── nexus-config/              Layered configuration
├── nexus-context/             Discovery, instructions, Git context, search
├── nexus-agents/              Scheduling, task graphs, coordinator, observability
├── nexus-models/              Provider contracts and adapters
├── nexus-tools/               Tool registry and local tools
├── nexus-permissions/         Policies and approvals
├── nexus-runtime/             Agent loop, cancellation, audit events
├── nexus-storage/             SQLite sessions and replay
├── nexus-workspace/           Worktrees + container lifecycle isolation
├── nexus-mcp/                 MCP client and Tool adapters
├── nexus-skills/              Skills, templates, and permissioned hooks
├── nexus-plugins/             Plugin discovery, runtime, capabilities, audit
├── nexus-teams/               Bot teams: roster, channels, task board, floor control,
│                              governance, mission engine, persistence, templates
└── nexus-sdk/                 Public embedding API
```

# 🗺️ Roadmap

## Completed
- [x] Phase 1 — Execution Core
- [x] Phase 2 — Context + Sessions
- [x] Phase 3 — Multi-Agent Orchestration foundation

## Current: Phase 4
- [x] MCP transport and Nexus Tool adapters
- [x] Skills and templates
- [x] Permission-controlled hook execution seam
- [x] Plugin manifests and capability boundaries
- [x] Public SDK foundation
- [x] CLI integration for unified multi-agent orchestration
- [x] Structured run-level observability
- [x] Container lifecycle management
- [x] Plugin runtime loading and capability enforcement
- [x] Interactive TUI foundation
- [x] Live runtime/event stream binding for the TUI
- [x] TUI command dispatch and cancellation controls
- [ ] Tauri desktop application
- [ ] Remote workers

## Nexus Teams
- [x] Bot archetypes, project bots, teams, and templates
- [x] Channels, direct messages, threads, mentions, and a shared task board
- [x] Floor policies and parallel turns
- [x] Approval governance (all / majority / any, human sign-off)
- [x] Budgets, stall detection, timeouts, pause/resume, cancellation
- [x] Event-sourced persistence, replay, transcripts, reports, and resume
- [x] Model-backed bots with permissioned tools and per-bot worktrees
- [x] CLI and TUI operator consoles
- [ ] Bot memory that carries across missions
- [ ] Teams that delegate sub-goals to other teams
- [ ] Scheduled and recurring missions
- [ ] Web dashboard and chat bridges (Slack, Microsoft Teams)

# 🛠️ Development Workflow

```bash
cargo fmt --all
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

# 💭 The Nexus Principle

> **Don't build an AI assistant that locks developers into one workflow. Build the primitives that let developers create their own.**
