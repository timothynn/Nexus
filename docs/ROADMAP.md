# Nexus Roadmap

## Current priority order

1. **Execution core** — model contracts, tool loops, permissions, auditability, cancellation.
2. **Workspace isolation** — Git worktrees and one workspace per parallel agent.
3. **Real model providers** — OpenAI-compatible first, then additional adapters.
4. **Configuration and sessions** — project settings, durable runs, replay, and diagnostics.
5. **Context engineering** — repository discovery, instructions, indexing, token budgets, inspection.
6. **Parallel agents** — orchestration over isolated workspaces.
7. **Extensibility** — MCP, skills, hooks, plugins, and SDKs.
8. **Interfaces** — TUI, desktop, IDE integration, and remote workers.

## Implemented

### Execution core
- [x] Provider-neutral model contracts and streaming
- [x] OpenAI-compatible provider
- [x] Bounded model-driven tool loop
- [x] Structured filesystem and shell tools
- [x] Explicit allow / ask / deny / sandbox policies
- [x] CLI approval prompts
- [x] Cooperative runtime cancellation boundary
- [x] Structured execution audit events

### Workspace isolation
- [x] Git worktree lifecycle
- [x] Isolated `nexus/<name>` branches
- [x] Safe removal with no automatic merge
- [x] One-worktree-per-agent allocation API
- [x] Partial-allocation rollback
- [ ] Concurrent subagent scheduler executing assigned tasks
- [ ] Automatic cleanup policy

### Configuration and persistence
- [x] Built-in defaults
- [x] User configuration discovery
- [x] Project `.nexus/config.toml` discovery
- [x] Environment overrides
- [x] CLI configuration inspection
- [x] SQLite session storage
- [x] Ordered event replay
- [x] CLI `--session` persistence and `replay`
- [ ] Fine-grained field-level layered merge diagnostics

### Context engineering
- [x] Deterministic repository discovery
- [x] Ignore `.git`, `.nexus`, `target`, and `node_modules`
- [x] File-size limits
- [x] Token-budget truncation
- [x] CLI context inspection
- [ ] Hierarchical instructions
- [ ] Symbol/code indexing
- [ ] Git-aware context selection
- [ ] Model-aware token accounting

## CLI surfaces

```text
nexus run --session "task"
nexus replay <session-id>
nexus context
nexus config
nexus worktree allocate-agents <run-name> <count>
```

## Next implementation block

1. Concurrent subagent scheduler on top of the existing `allocate_agents` seam.
2. Persist runtime audit events directly from controlled runs, including tool calls.
3. Wire OS cancellation signals into the runtime cancellation token.
4. Hierarchical instructions and Git-aware context selection.
5. Code indexing/search.
6. MCP client support.
7. Skills and hooks.
8. TUI and desktop interfaces.
