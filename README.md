<div align="center">

# ◈ Nexus

### The programmable AI harness for developers, agents, and autonomous workflows.

**Local-first · Model-agnostic · Tool-native · Workspace-isolated · Observable by design**

[**Quick Start**](#-quick-start) · [**Working Now**](#-whats-working-now) · [**Architecture**](#-architecture) · [**Roadmap**](#-roadmap)

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

For a real provider:

```bash
export OPENAI_API_KEY="..."
nexus run --provider openai-compatible --model "your-model-id" --tools --session "inspect this repository"
```

## Multi-agent orchestration

Run independent agents in isolated worktrees:

```bash
nexus agents run "inspect this repository" 3 --concurrency 2 --tools
```

Or execute a dependency-aware graph. Task syntax is `task:dependency1,dependency2`:

```bash
nexus agents graph \
  research \
  implement:research \
  tests:implement \
  review:tests \
  --concurrency 2 --tools
```

Nexus executes dependency layers, collects typed worker handoffs, then runs a supervisor and reviewer without automatically merging changes.

## Embed Nexus in Rust

Nexus now has a small public SDK facade for embedding the harness into another Rust application:

```rust
use std::sync::Arc;

use nexus_models::MockModelProvider;
use nexus_sdk::{Nexus, RunRequest};

let nexus = Nexus::builder()
    .provider(Arc::new(MockModelProvider::default()))
    .model("mock-1")
    .build()?;

let result = nexus.run(RunRequest::new("inspect this repository")).await?;
println!("{}", result.message);
```

The SDK intentionally starts small: builder-based configuration, provider/model injection, cancellable execution, and access to the advanced runtime seam.

# 🟢 What's Working Now

## Phase 1 — Execution Core ✅

- Provider-neutral models, streaming, and OpenAI-compatible adapter
- Bounded model-driven tool loop
- Filesystem and structured shell tools
- `allow` / `ask` / `deny` / `sandbox` permissions
- CLI approval boundary and Ctrl+C cancellation
- Structured audit events
- SQLite session persistence and replay
- Isolated Git worktrees

## Phase 2 — Context + Sessions ✅

- Layered global/project/environment configuration
- Deterministic repository discovery and context budgets
- Hierarchical `.nexus/instructions.md` and `AGENTS.md`
- Git-aware prioritization and local code search
- Model-aware token estimates
- Durable sessions and replay

## Phase 3 — Multi-Agent Orchestration ✅

```text
Task Graph → Dependency Layers → Parallel Workers
                                  ↓
                           Typed Handoffs
                                  ↓
                        Supervisor → Reviewer
                                  ↓
                       Explicit Human Review
```

Implemented: isolated worktrees, bounded concurrency, deterministic results, task graphs, unified coordination, cancellation fan-out, worker/supervisor/reviewer handoffs, cleanup policies, review candidates, explicit merge boundaries, container execution foundation, and a CLI graph operator surface.

### Run-level observability

Parallel orchestration now emits structured lifecycle events:

```text
run.started
layer.started
worker.started
worker.completed
worker.failed
role.started
role.completed
run.completed
run.cancelled
```

The `AgentEventSink` seam is shared by both `ParallelAgentScheduler` and `MultiAgentCoordinator`, making it possible to plug the same event stream into a future TUI, desktop timeline, SQLite run history, tracing backend, or remote-worker dashboard without changing orchestration logic.

## Phase 4 — Extensibility 🚧

### MCP tools use the Nexus tool boundary

```text
MCP Server → tools/list → MCP Tool Adapter → ToolRegistry
                                         ↓
                                  Permissions + Audit
```

### Skills, templates, and hooks

```text
.nexus/skills/<name>/SKILL.md
.nexus/agents/<name>.toml
.nexus/hooks.toml
```

Hooks have a permission-aware execution seam. Every configured lifecycle command must pass a `PermissionPolicy`; denied or approval-required hooks cannot execute silently.

### Plugins and capability boundaries

Project plugins live under:

```text
.nexus/plugins/<name>/plugin.toml
```

Each manifest declares its entrypoint and explicit capabilities, such as filesystem access, shell execution, network access, model access, workspace management, and session access.

### Public SDK foundation

`nexus-sdk` is the stable embedding facade for applications that want to construct and run Nexus without coupling directly to the CLI.

# 🧬 Architecture

```text
apps/
└── nexus-cli/                 Operator surfaces

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
├── nexus-workspace/           Worktrees, review, cleanup, container backend
├── nexus-mcp/                 MCP client and Tool adapters
├── nexus-skills/              Skills, templates, and permissioned hooks
├── nexus-plugins/             Plugin manifests and capability boundaries
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
- [x] Structured run-level observability across parallel workers
- [ ] Container lifecycle management
- [ ] Plugin runtime loading and capability enforcement
- [ ] Interactive TUI
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
