# Nexus Roadmap

## Current priority order

Nexus is being implemented as a reliable execution harness before expanding into desktop UI or broad automation features:

1. **Execution core** — model contracts, tool registry, permissions, auditability, cancellation.
2. **Workspace isolation** — Git worktrees for safe parallel coding agents.
3. **Real model providers** — OpenAI-compatible first, then additional local/cloud adapters.
4. **Context engineering** — repository discovery, instructions, indexing, token budgets, context inspection.
5. **Sessions and replay** — durable runs, tool history, diffs, metrics, and reproducibility.
6. **Parallel agents** — subagents orchestrated over isolated workspaces.
7. **Extensibility** — MCP, skills, hooks, plugins, and SDKs.
8. **Interfaces** — TUI, desktop, IDE integration, and remote workers.

## Phase 1 — Core MVP

### Milestone 1: Bootstrap
- [x] Initialize repository
- [x] Create Rust workspace
- [x] Define initial crate boundaries
- [x] Add initial CLI
- [x] Add architecture documentation
- [x] Add CI baseline

### Milestone 2: Agent and model contracts
- [x] Provider trait
- [x] Model capabilities
- [x] Streaming events
- [x] Mock provider
- [x] Basic runtime model execution
- [x] Streaming runtime execution
- [ ] Agent configuration
- [ ] First real provider adapter
- [ ] Provider registry in the CLI

### Milestone 3: Tools and permissions
- [x] Tool registry
- [x] Tool metadata and lifecycle contracts
- [x] Workspace-rooted filesystem read tool
- [x] Permission evaluation
- [x] Explicit allow / ask / deny / sandbox enforcement
- [ ] Shell tool with structured arguments and timeout
- [ ] Interactive approval flow
- [ ] Execution audit event store
- [ ] Tool loop integrated into model-driven agent execution
- [ ] Cancellation propagation

### Milestone 4: Workspace isolation
- [x] Dedicated workspace abstraction boundary
- [x] Git worktree creation on isolated `nexus/<name>` branches
- [x] Worktree listing
- [x] Worktree status and diff inspection
- [x] Safe worktree removal with no automatic merge
- [ ] Workspace metadata persistence
- [ ] Per-run workspace assignment
- [ ] Automatic cleanup policies
- [ ] Parallel-agent workspace allocation
- [ ] Container/remote workspace implementations

### Milestone 5: Configuration and sessions
- [x] Initial configuration boundary
- [ ] Global configuration
- [ ] Project `.nexus/` configuration
- [ ] Precedence and diagnostics
- [ ] SQLite sessions
- [ ] Run history
- [ ] Replayable execution traces

### Milestone 6: First usable vertical slice

```text
Task
  → Config
  → Agent Runtime
  → Model
  → Permission Check
  → Tool Registry
  → Tool Execution
  → Run Events
  → Session
  → Result
```

The first release target is a genuinely usable CLI before expanding into TUI, desktop, MCP, plugins, workflows, or multi-agent orchestration.

## Phase 2 — Developer Harness

- [ ] OpenAI-compatible provider
- [ ] Repository context discovery
- [ ] Hierarchical instructions
- [ ] Code search and indexing
- [ ] Git-aware context and diff review
- [ ] Context inspector and token budgets
- [ ] Local session persistence and replay
- [ ] TUI
- [ ] Agent templates
- [ ] MCP client support
- [ ] Skills and hooks
- [ ] Model routing and fallback

## Phase 3 — Reliable Agent Platform

- [ ] Model-driven tool loop
- [ ] Parallel subagents
- [ ] One isolated workspace per coding agent
- [ ] Supervisor and reviewer patterns
- [ ] Task graphs and dependencies
- [ ] Structured outputs
- [ ] Retry, timeout, and cancellation policies
- [ ] Sandboxed execution backends
- [ ] Secret redaction and audit logs

## Phase 4 — Nexus Desktop and Ecosystem

- [ ] Tauri desktop application
- [ ] Customizable workspace
- [ ] Visual agent management
- [ ] Context inspector
- [ ] Run replay
- [ ] Plugin manager
- [ ] Workflow editor
- [ ] Public SDKs
- [ ] Remote workers
- [ ] Plugin ecosystem
- [ ] Skill registry
- [ ] Team collaboration
