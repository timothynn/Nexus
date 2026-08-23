# Nexus Roadmap

Nexus is developed as a set of composable runtime primitives rather than one monolithic assistant. Each phase should leave behind reusable contracts for the phases that follow.

## Phase 1 — Execution Core ✅

### Implemented

- [x] Rust workspace and explicit crate boundaries
- [x] Provider-neutral model contracts and streaming interfaces
- [x] Deterministic mock provider
- [x] OpenAI-compatible Chat Completions provider
- [x] Model capabilities and provider checks
- [x] Tool registry with JSON schemas
- [x] Workspace-rooted filesystem tool
- [x] Structured shell execution with timeout policy
- [x] `allow` / `ask` / `deny` / `sandbox` permissions
- [x] CLI approval boundary
- [x] Bounded model-driven tool loop
- [x] Aggregated token usage across tool steps
- [x] Cooperative runtime cancellation
- [x] OS Ctrl+C cancellation for controlled CLI runs
- [x] Structured audit events
- [x] Tool failure audit events
- [x] SQLite-backed audit persistence
- [x] Ordered session replay
- [x] Git worktree lifecycle

### Phase 1 exit criteria

The harness can execute a bounded agent run, enforce permissions before tools execute, accept cancellation, and persist/replay a structured execution trace.

**Status: complete.**

---

## Phase 2 — Context + Sessions ✅

### Implemented

- [x] Built-in defaults
- [x] User configuration discovery
- [x] Project `.nexus/config.toml` discovery
- [x] Environment overrides
- [x] CLI configuration inspection
- [x] SQLite session creation
- [x] Ordered event replay
- [x] Deterministic repository traversal
- [x] Ignore `.git`, `.nexus`, `target`, and `node_modules`
- [x] File-size limits and binary-file avoidance
- [x] Token-budget truncation
- [x] Context inspection CLI
- [x] Hierarchical `.nexus/instructions.md` and `AGENTS.md`
- [x] Nested root-to-leaf instruction resolution
- [x] Agent-template and selected-skill instruction composition
- [x] Git-aware modified/staged/untracked file prioritization
- [x] Lightweight deterministic code indexing and search
- [x] Model-family token estimates, explicitly labelled approximate

### Phase 2 exit criteria

Nexus can explain what repository context and instructions are being considered, prioritize current Git changes, enforce context budgets, search local code, and persist/replay runs.

**Status: complete.**

---

## Phase 3 — Parallel Agents 🟡

### Implemented foundation

- [x] One isolated worktree per agent
- [x] Isolated `nexus/<workspace>` branches
- [x] Partial-allocation rollback
- [x] Bounded concurrent scheduler
- [x] Deterministic result ordering
- [x] CLI command for real Nexus runs inside isolated workspaces
- [x] Unique worktree run names to avoid repeated-run collisions

### Next

- [ ] Supervisor and reviewer agent patterns
- [ ] Task graphs and dependencies
- [ ] Shared run coordinator and cancellation fan-out
- [ ] Agent result aggregation strategies
- [ ] Automatic workspace cleanup policies
- [ ] Sandboxed/container workspace backends
- [ ] Explicit human review/merge workflow

---

## Phase 4 — Extensibility + Interfaces 🟡

### Implemented foundation

#### MCP

- [x] Dedicated `nexus-mcp` crate
- [x] Stdio subprocess transport
- [x] JSON-RPC request/response handling
- [x] Client initialization
- [x] `tools/list`
- [x] `tools/call`

#### Skills and templates

- [x] Project-local `.nexus/skills/<name>/SKILL.md` discovery
- [x] Explicit skill loading
- [x] `.nexus/agents/<name>.toml` templates
- [x] Template instruction composition
- [x] Template-selected skills

#### Hooks

- [x] Project-local `.nexus/hooks.toml` configuration
- [x] Lifecycle event model
- [x] Hook inspection CLI
- [ ] Permission-controlled hook execution

### Next

1. Bridge MCP tools into `ToolRegistry` with permission checks and audit events.
2. Execute configured hooks through structured shell requests rather than implicit command execution.
3. Add plugin manifests, discovery, compatibility checks, and capability boundaries.
4. Add public Rust/TypeScript SDK seams.
5. Build a TUI around run traces, approvals, context inspection, and workspaces.
6. Build the Tauri desktop shell using the same runtime contracts.
7. Add remote worker/workspace backends.

---

## CLI surfaces

```text
nexus run --tools --session --git-context
nexus replay <session-id>
nexus context --git-aware --model <name>
nexus search <query>
nexus instructions [path]
nexus skills list|show|hooks|template
nexus mcp list-tools|call
nexus agents run <task> <count>
nexus worktree allocate-agents <run-name> <count>
```

## Immediate implementation priority

The next vertical slice should be **MCP tool bridging**:

```text
MCP server
    ↓
tools/list
    ↓
MCP tool adapter
    ↓
Nexus ToolRegistry
    ↓
Permission policy
    ↓
Audit event
    ↓
Model-driven agent loop
```

That makes the existing MCP foundation useful to agents instead of limiting it to direct CLI inspection.
