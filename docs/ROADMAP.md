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

### Next
- [ ] Shared run coordinator with cancellation fan-out
- [ ] Execute supervisor/reviewer plans as first-class runtime workflows
- [ ] Result aggregation strategies and structured handoffs
- [ ] Container workspace backend
- [ ] Explicit human review/merge command

## Phase 4 — Extensibility + Interfaces 🟡

### Implemented
- [x] MCP stdio JSON-RPC client
- [x] `tools/list` and `tools/call`
- [x] MCP tool adapters that implement Nexus `Tool`
- [x] Project-local skills and agent templates
- [x] Hook configuration and lifecycle model

### Next
- [ ] Register MCP adapters from configured servers into runtime tool registries
- [ ] Permission-controlled hook execution through structured shell requests
- [ ] Plugin manifests, discovery, compatibility checks, and capability boundaries
- [ ] Public SDK seams
- [ ] TUI for runs, approvals, context, and workspaces
- [ ] Tauri desktop shell
- [ ] Remote workers and workspace backends

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

Nexus must continue to preserve explicit permissions, isolated workspaces, observable execution, and no silent autonomous merges.
