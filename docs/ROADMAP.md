# Nexus Roadmap

## Current priority order

Nexus is being implemented as a reliable execution harness before expanding into desktop UI or broad automation features:

1. **Execution core** — model contracts, tool loops, permissions, auditability, cancellation.
2. **Workspace isolation** — Git worktrees for safe parallel coding agents.
3. **Real model providers** — OpenAI-compatible first, then additional local/cloud adapters.
4. **Configuration and sessions** — project settings, durable runs, replay, and diagnostics.
5. **Context engineering** — repository discovery, instructions, indexing, token budgets, context inspection.
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
- [x] Provider-neutral tool definitions
- [x] Provider-neutral tool call responses
- [x] Tool result messages with call correlation
- [x] Mock provider
- [x] Basic runtime model execution
- [x] Streaming runtime execution
- [x] Bounded model-driven tool loop
- [ ] Agent configuration
- [ ] First real provider adapter
- [ ] Provider registry in the CLI

### Milestone 3: Tools and permissions
- [x] Tool registry
- [x] Tool metadata and JSON schemas
- [x] Workspace-rooted filesystem read tool
- [x] Structured shell tool with direct program/argument execution
- [x] Workspace-bounded shell working directory
- [x] Tool timeout limits
- [x] Permission evaluation
- [x] Explicit allow / ask / deny / sandbox enforcement
- [x] Pluggable approval boundary for `ask`
- [x] Runtime enforcement before every tool execution
- [ ] CLI/desktop approval UX
- [ ] Execution audit event store
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
  → Tool Call?
      ├── no → Result
      └── yes
            → Permission Check
            → Approval Boundary (when needed)
            → Tool Execution
            → Tool Result
            → Model
```

The next milestone is making this vertical slice usable from the CLI with a real provider, built-in workspace tools, and durable execution traces.

## Phase 2 — Developer Harness

- [ ] OpenAI-compatible provider
- [ ] Provider configuration and environment diagnostics
- [ ] Built-in coding agent tool bundle
- [ ] CLI approval prompts
- [ ] Repository context discovery
- [ ] Hierarchical instructions
- [ ] Code indexing and search
- [ ] Git-aware context and diff review
- [ ] Context inspector and token budgets
- [ ] Local session persistence and replay
- [ ] TUI
- [ ] Agent templates
- [ ] MCP client support
- [ ] Skills and hooks
- [ ] Model routing and fallback

## Phase 3 — Reliable Agent Platform

- [x] Model-driven tool loop foundation
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
