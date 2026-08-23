# Nexus Codebase Structure Summary

## Overall Project Layout

```
Nexus/
├── Cargo.toml          # Workspace root with 14 members
├── README.md           # Project overview and quick start
├── docs/
│   ├── ARCHITECTURE.md # High-level architecture and dependency direction
│   ├── ROADMAP.md      # Phase-based roadmap (Phase 1-4)
│   └── agents/         # Agent/triage documentation
├── apps/
│   └── nexus-cli/      # CLI entry point
└── crates/             # 14 library crates
```

## Crate Structure and Purpose

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `nexus-core` | Stable domain contracts (SessionId, Task) | serde, thiserror, uuid |
| `nexus-config` | Configuration loading, validation, precedence | dirs, serde, toml |
| `nexus-context` | Repository discovery, instructions, Git-aware selection, code search | fs, serde, tokio |
| `nexus-agents` | Parallel scheduling, task graphs, agent orchestration | nexus-workspace |
| `nexus-models` | Provider-neutral model contracts, Mock/OpenAI providers | reqwest, serde, async-trait |
| `nexus-tools` | Tool registry, built-in FileSystemTool, ShellTool | tokio, serde |
| `nexus-permissions` | Explicit allow/ask/deny/sandbox decisions | serde |
| `nexus-runtime` | Agent execution, tool loop, cancellation, audit events | all other crates |
| `nexus-storage` | SQLite session and event persistence | rusqlite |
| `nexus-workspace` | Git worktree-based isolated workspaces | git2 |
| `nexus-mcp` | MCP stdio JSON-RPC client, tool adapters | tokio, serde |
| `nexus-skills` | Project-local skills, hooks, agent templates | fs, serde |
| `nexus-workspace` | Git worktree-based isolated workspaces | git2 |

### Detailed Crate Responsibilities (from ARCHITECTURE.md)

- **nexus-cli**: User-facing commands and terminal output
- **nexus-core**: Stable IDs, tasks, events, and shared domain contracts
- **nexus-config**: Configuration loading, validation, precedence, diagnostics
- **nexus-models**: Provider/model contracts and future routing
- **nexus-tools**: Tool definitions, registry, execution lifecycle
- **nexus-permissions**: Allow/ask/deny/sandbox decisions
- **nexus-runtime**: Agent execution and orchestration
- **nexus-storage**: Session and execution persistence
- **nexus-workspace**: Worktrees and workspace policies
- **nexus-mcp**: MCP client and Tool adapters
- **nexus-skills**: Skills, hooks, and templates

## Dependency Direction (from ARCHITECTURE.md)

```
nexus-cli → nexus-runtime → nexus-core

Implementations plug into runtime contracts:
  config · models · tools · permissions · storage
```

**Phase 1 rule**: `nexus-core` must remain provider-neutral and UI-neutral. The CLI orchestrates user interaction but does not own agent behavior. The runtime coordinates execution without depending on a specific AI provider.

## Main Entry Points

### CLI (`apps/nexus-cli/src/main.rs`)

The main entry point with the following command categories:

- `run` - Execute a task with optional tools, model, provider
- `models` - List built-in providers (mock, openai-compatible)
- `config` - Display resolved configuration
- `context` - Inspect repository context and token budgets
- `search` - Search code index
- `instructions` - Show hierarchical instruction chain
- `skills` - List/show/manage skills and templates
- `mcp` - Interact with MCP servers
- `agents` - Execute multiple parallel agents in worktrees
- `replay` - Replay persisted execution events
- `doctor` - Summary of available components
- `worktree` - Git worktree management

### Key Types and Architectural Patterns

#### Domain Contracts (`nexus-core`)
- `SessionId` (UUID-based) - stable session identifiers
- `Task` (id + prompt) - units of work

#### Model Contracts (`nexus-models`)
- `ModelProvider` trait - provider-neutral interface with `complete()` and `stream()`
- `ModelRequest` / `ModelResponse` - neutral request/response types
- `ModelStreamEvent` - streaming events (Started, Delta, Completed)
- `MockModelProvider` - deterministic local provider for tests
- `OpenAiCompatibleProvider` - HTTP adapter for compatible endpoints

#### Tool Contracts (`nexus-tools`)
- `Tool` trait - `metadata()` + `execute(request)`
- `ToolRegistry` - registered tool management with deduplication
- `FileSystemTool` - read-only files rooted in workspace
- `ShellTool` - structured program execution (no shell interpolation)
- `ToolError` - execution, timeout, not found, invalid input errors

#### Permission System (`nexus-permissions`)
- `PermissionDecision` - Allow, Ask, Deny, Sandbox
- `PermissionPolicy` trait - `evaluate(request)`
- `RuleBasedPolicy` - rules with wildcard support (`filesystem.*`)
- `PermissionApprover` - human/boundary approval for "ask" decisions
- Explicit `enforce()` and `enforce_with_approver()` functions

#### Runtime Orchestration (`nexus-runtime`)
- `AgentRuntime` - model invocation with tool support
- `AuthorizedToolExecutor` - executes tools only after policy approval
- Cooperative cancellation via `CancellationToken`
- Structured `AuditEvent` recording via `AuditSink`
- `RuntimeError` enum with Model, Permission, Tool, Cancelled, MaxToolSteps variants

#### Storage (`nexus-storage`)
- `SqliteStore` - SQLite-backed session and event persistence
- `SessionStore` trait - abstract interface
- Append/replay events with sequence numbering

#### Workspace Isolation (`nexus-workspace`)
- `GitWorktreeManager` - Git worktree-based isolated workspaces
- `AgentWorkspace` - workspace with agent index and worktree
- `CleanupPolicy` - Keep, RemoveClean, RemoveAlways
- Worktree creation, listing, removal, diff, status

#### MCP Integration (`nexus-mcp`)
- `StdioMcpClient` - stdio JSON-RPC client for MCP servers
- `McpToolAdapter` - adapts MCP tools into Nexus `Tool` implementations
- `discover_tool_adapters()` - discover and adapt MCP tools

#### Skills and Templates (`nexus-skills`)
- Skills live under `.nexus/skills/<name>/SKILL.md`
- Agent templates live in `.nexus/agents/<name>.toml`
- Hook configuration in `.nexus/hooks.toml`
- `HookEvent` enum: RunStarted, BeforeModel, BeforeTool, AfterTool, RunCompleted, RunFailed

## Architectural Patterns

### 1. Provider-Neutral Domain Contracts
- `nexus-core` types (SessionId, Task) are framework-independent
- `nexus-models` defines the `ModelProvider` trait boundary
- Concrete providers (Mock, OpenAiCompatible) implement the trait at the boundary

### 2. Explicit Permission Boundaries
- Permissions are explicit decisions (Allow/Ask/Deny/Sandbox)
- `enforce()` function returns error for Ask/Deny/Sandbox
- `enforce_with_approver()` allows human approval for "ask" decisions
- CLI uses `StdinApprover` for interactive approval, `ApproveAll` for CI

### 3. Git-Aware Workspace Isolation
- Each parallel agent gets an isolated Git worktree
- Worktrees are created under `.nexus/worktrees/`
- Deterministic allocation with `allocate_agents()`
- Cleanup policies: Keep, RemoveClean, RemoveAlways
- Filesystem and shell tools are rooted within worktree paths

### 4. Bounded Tool Loop with Cancellation
- `run_with_tools_controlled()` with max_steps limit
- Cooperative cancellation via `tokio::select!` on `CancellationToken`
- Audit events at each step (model.requested, tool.requested, tool.completed, run.*)
- Max tool steps error prevents infinite loops

### 5. Hierarchical Configuration Loading
- Config loaded from multiple sources with precedence:
  1. Built-in defaults
  2. `.nexus/config.toml` in project root
  3. `$XDG_CONFIG_HOME/nexus/config.toml`
  4. `NEXUS_DEFAULT_AGENT` env var
  5. `NEXUS_MAX_STEPS` env var
- `ResolvedConfig` tracks sources for auditability

### 6. Instruction Composition
- Hierarchical `.nexus/AGENTS.md` files (root → target directory)
- `.nexus/instructions.md` as project-level instructions
- Skills loaded from `.nexus/skills/<name>/SKILL.md`
- Agent templates from `.nexus/agents/<name>.toml`
- Combined instructions via `InstructionSet::combined()`

### 7. Deterministic Code Indexing and Search
- `ContextSnapshot` with token-budget-aware file selection
- `CodeIndex` builds searchable index from context files
- `search()` ranks matches by term frequency, then path, then line number

## Documentation

### Existing Docs
- `docs/ARCHITECTURE.md` - High-level architecture and dependency direction
- `docs/ROADMAP.md` - Phase-based roadmap (Phase 1-4, with checkmarks)
- `docs/agents/domain.md` - Single-context layout convention
- `docs/agents/issue-tracker.md` - GitHub Issues usage
- `docs/agents/triage-labels.md` - Canonical triage labels

### ADRs
- No `docs/adr/` directory exists yet (as confirmed by glob search)
- `docs/agents/domain.md` references that ADRs live in `docs/adr/`

### No ADRs Found
The glob search for `docs/adr/**/*` returned no results, indicating no Architecture Decision Records have been written yet.

## Test Structure

Tests are embedded within each crate's `lib.rs` file (module `mod tests`):

- **nexus-core**: No tests
- **nexus-config**: `defaults_are_stable` test
- **nexus-context**: `estimates_non_empty_text`, `search_returns_ranked_matches`
- **nexus-agents**: `graph_builds_dependency_layers`
- **nexus-models**: 7 tests (mock provider, registry, tool metadata, OpenAI compat requests/responses, endpoint building)
- **nexus-tools**: 4 tests (registry execution, duplicate rejection, shell tool, timeout policy)
- **nexus-permissions**: 5 tests (exact rule, wildcard, ask not silently allowed, approver allow, approver reject)
- **nexus-storage**: `events_can_be_replayed_in_order`
- **nexus-mcp**: `adapter_namespaces_remote_tools`
- **nexus-skills**: `missing_hook_returns_empty_command_list`
- **nexus-workspace**: `workspace_names_reject_paths`, `cleanup_policy_is_explicit`

### Test Organization
- Tests use `#[tokio::test]` for async tests
- Tests use `#[test]` for sync tests
- Uses `thiserror` for error type testing
- Uses `serde_json` for JSON test fixtures
- Uses `tokio` for async runtime

### Development Workflow (from README)
```
cargo fmt --all
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Key Design Decisions

1. **No concrete providers in core**: `nexus-core` has zero provider-dependent dependencies
2. **Explicit over implicit**: All permission decisions are explicit; hooks return commands for caller to execute
3. **Git-backed isolation**: Parallel agents get isolated worktrees, not just in-memory isolation
4. **SQLite persistence**: Sessions and audit events persisted to SQLite for replay
5. **Model-agnostic contracts**: `ModelProvider` trait enables swapping providers without core changes
6. **Tool sandboxing**: Tools rooted in workspace paths; absolute paths rejected
7. **Cancellation-driven**: Cooperative cancellation throughout (model calls, tool execution)
8. **Deterministic ordering**: Task graphs produce deterministic dependency layers; agent results sorted by index

## Missing / To Be Added

- No `docs/adr/` directory exists yet
- No `CONTEXT.md` at repo root (per `docs/agents/domain.md`, this is the single-context layout convention)
- Phase 3 remaining items: unified coordinator, cancellation fan-out, structured handoffs
- Phase 4 items: plugin system, TUI, remote workers