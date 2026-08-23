<div align="center">

# ◈ Nexus

### The programmable AI harness for developers, agents, and autonomous workflows.

**Local-first · Model-agnostic · Tool-native · Workspace-isolated · Observable by design**

[**Quick Start**](#-quick-start) · [**Working Now**](#-whats-working-now) · [**Architecture**](#-architecture) · [**Roadmap**](#-roadmap)

</div>

---

> **Nexus is not another AI chat wrapper.** It is a configurable runtime where models, agents, tools, permissions, context, sessions, workspaces, skills, hooks, and integrations are composable primitives.

# 🚀 Quick Start

```bash
git clone https://github.com/timothynn/Nexus.git
cd Nexus
cargo build --workspace
cargo run -p nexus-cli -- run "inspect this repository"
```

For a real provider:

```bash
export OPENAI_API_KEY="..."
nexus run --provider openai-compatible --model "your-model-id" --tools --session "inspect this repository"
```

# 🟢 What's Working Now

## Phase 1 — Execution Core ✅

```text
Task → Instructions/Context → Model → Tool Call
                              ↓
                    Permission Decision
                              ↓
                    Tool + Audit Event
                              ↓
                       Continue / Finish
```

Implemented:

- Provider-neutral models, streaming, and OpenAI-compatible adapter
- Bounded model-driven tool loop
- Filesystem and structured shell tools
- `allow` / `ask` / `deny` / `sandbox` permissions
- CLI approval boundary
- Ctrl+C cooperative cancellation
- Structured audit events
- SQLite session persistence and replay
- Isolated Git worktrees

## Phase 2 — Context + Sessions ✅

- Layered global/project/environment configuration
- Deterministic repository discovery
- Context and token budgets
- Hierarchical `.nexus/instructions.md` and `AGENTS.md`
- Git-aware modified/staged/untracked prioritization
- Local deterministic code search
- Model-aware token estimates
- Durable sessions and replay

## Phase 3 — Parallel Agents 🟡

Nexus now has the core orchestration primitives:

```text
Task Graph
   ↓
Dependency Layers
   ↓
Parallel Workers ── each in an isolated worktree
   ↓
Supervisor Plan
   ↓
Reviewer Plan
   ↓
Explicit Human Review / Merge
```

Implemented:

- One isolated worktree per agent
- Bounded concurrent scheduling
- Real Nexus runs in agent workspaces
- Deterministic result ordering
- Task graphs and dependency validation
- Supervisor / worker / reviewer plans
- Workspace cleanup policies: `keep`, `remove-clean`, `remove-always`
- Workspace backend abstraction for future containers and remote workers

The next Phase 3 step is executing these plans as a unified coordinator with cancellation fan-out and structured handoffs.

## Phase 4 — Extensibility 🟡

### MCP tools now use the Nexus tool boundary

```text
MCP Server
   ↓ tools/list
MCP Tool Adapter
   ↓
Nexus ToolRegistry
   ↓
Permission Policy
   ↓
Audit + Agent Loop
```

The `nexus-mcp` crate now adapts discovered MCP tools into Nexus `Tool` implementations. MCP tools can therefore inherit the same permission and execution contracts as built-in tools rather than becoming a parallel execution system.

### Skills, templates, and hooks

```text
.nexus/skills/<name>/SKILL.md
.nexus/agents/<name>.toml
.nexus/hooks.toml
```

Skills and templates are composable instruction sources. Hooks remain intentionally explicit configuration; automatic execution is the next step and must route through the same structured shell and permission boundaries.

# 🧬 Architecture

```text
apps/
└── nexus-cli/                 Operator surfaces

crates/
├── nexus-core/                Stable domain contracts
├── nexus-config/              Layered configuration
├── nexus-context/             Discovery, instructions, Git context, search
├── nexus-agents/              Parallel scheduling and task graphs
├── nexus-models/              Provider contracts and adapters
├── nexus-tools/               Tool registry and local tools
├── nexus-permissions/         Policies and approvals
├── nexus-runtime/             Agent loop, cancellation, audit events
├── nexus-storage/             SQLite sessions and replay
├── nexus-workspace/           Worktrees and workspace policies
├── nexus-mcp/                 MCP client and Tool adapters
└── nexus-skills/              Skills, hooks, and templates
```

# 🗺️ Roadmap

## Completed
- [x] Phase 1 — Execution Core
- [x] Phase 2 — Context + Sessions

## Current: Phase 3
- [x] Parallel workers
- [x] Task graph contracts
- [x] Supervisor/reviewer plans
- [x] Cleanup policies
- [ ] Unified coordinator and cancellation fan-out
- [ ] Structured supervisor/reviewer handoffs
- [ ] Container backend
- [ ] Human review/merge workflow

## Next: Phase 4
- [x] MCP transport and Nexus Tool adapters
- [x] Skills and templates
- [x] Hook configuration
- [ ] Permission-controlled hook execution
- [ ] Plugin system and SDK seams
- [ ] TUI
- [ ] Tauri desktop application
- [ ] Remote workers

# 🛠️ Development Workflow

```bash
cargo fmt --all
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

# 💭 The Nexus Principle

> **Don't build an AI assistant that locks developers into one workflow. Build the primitives that let developers create their own.**

Nexus is designed so the same execution contracts can power a CLI today, a TUI tomorrow, and a desktop or distributed agent platform later—without rewriting the core runtime.
