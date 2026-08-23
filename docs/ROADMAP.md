# Nexus Roadmap

## Phase 1 — Core MVP

### Milestone 1: Bootstrap
- [x] Initialize repository
- [x] Create Rust workspace
- [x] Define initial crate boundaries
- [x] Add initial CLI
- [x] Add architecture documentation
- [x] Add CI baseline

### Milestone 2: Agent and model contracts
- [ ] Provider trait
- [ ] Model capabilities
- [ ] Streaming events
- [ ] Mock provider
- [ ] Agent configuration

### Milestone 3: Tools and permissions
- [ ] Tool registry
- [ ] Filesystem tool
- [ ] Shell tool
- [ ] Permission evaluation
- [ ] Approval flow
- [ ] Execution audit events

### Milestone 4: Configuration and sessions
- [ ] Global configuration
- [ ] Project `.nexus/` configuration
- [ ] Precedence and diagnostics
- [ ] SQLite sessions
- [ ] Run history

### Milestone 5: Vertical slice

```text
Task → Config → Runtime → Model → Tool → Permission → Event → Session → Result
```

The first release target is a genuinely usable CLI before expanding into TUI, desktop, MCP, plugins, workflows, or multi-agent orchestration.
