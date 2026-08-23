# Nexus Architecture

## Dependency direction

```text
nexus-cli
    ↓
nexus-runtime
    ↓
nexus-core

Implementations plug into runtime contracts:
config · models · tools · permissions · storage
```

`nexus-core` must remain provider-neutral and UI-neutral. The CLI orchestrates user interaction but does not own agent behavior. The runtime coordinates execution without depending on a specific AI provider.

## Package responsibilities

| Package | Responsibility |
| --- | --- |
| `nexus-cli` | User-facing commands and terminal output |
| `nexus-core` | Stable IDs, tasks, events, and shared domain contracts |
| `nexus-config` | Configuration loading, validation, precedence, diagnostics |
| `nexus-models` | Provider/model contracts and future routing |
| `nexus-tools` | Tool definitions, registry, execution lifecycle |
| `nexus-permissions` | Allow/ask/deny/sandbox decisions |
| `nexus-runtime` | Agent execution and orchestration |
| `nexus-storage` | Session and execution persistence |

## Phase 1 rule

Do not add concrete providers, desktop UI, MCP implementations, or plugin machinery directly into the core runtime. Introduce them through explicit interfaces.
