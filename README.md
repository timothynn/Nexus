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

Launch the interactive operator console:

```bash
cargo run -p nexus-tui
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

### Interactive TUI: events + command dispatch + cancellation

`nexus-tui` is now a dedicated terminal operator surface with a bidirectional runtime bridge:

```text
Nexus runtime events ───────→ TUI timeline
                                  ↓
Operator command input ────→ command bridge
                                  ↓
                             runtime boundary
                                  ↑
Cancel control ────────────→ shared cancellation path
```

Current operator controls:

- `Tab` cycles focus
- `←` / `→` switches workspace tabs
- `↑` / `↓` inspects the event timeline
- `Enter` dispatches the command input through the runtime bridge
- `c` requests cancellation outside command mode
- `Esc` leaves command input
- `q` exits

The TUI remains intentionally thin: it uses Nexus event contracts and a runtime command boundary rather than recreating orchestration inside the UI.

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

# 🛠️ Development Workflow

```bash
cargo fmt --all
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

# 💭 The Nexus Principle

> **Don't build an AI assistant that locks developers into one workflow. Build the primitives that let developers create their own.**
