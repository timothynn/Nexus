# Nexus

**Nexus is a configurable, model-agnostic AI harness and agent runtime.**

The project is designed as a local-first, terminal-first platform that can grow into a desktop AI workspace, agent runtime, workflow engine, and extensibility platform.

## Current status

🚧 **Phase 1 bootstrap is in progress.**

The initial workspace establishes explicit boundaries for the CLI, runtime, model providers, tools, permissions, configuration, and storage.

## Architecture

```text
apps/
└── nexus-cli/            Command-line interface

crates/
├── nexus-core/           Stable domain types and contracts
├── nexus-config/         Configuration contracts and resolution
├── nexus-models/         Provider-neutral model contracts
├── nexus-tools/          Tool execution contracts
├── nexus-permissions/    Permission policy contracts
├── nexus-runtime/        Agent execution loop
└── nexus-storage/        Session persistence contracts
```

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run the CLI:

```bash
cargo run -p nexus-cli -- run "inspect this repository"
```

## Principles

- Local-first
- Model-agnostic
- Transparent execution
- Explicit permissions
- Modular architecture
- Configurable by default
- Extensible by design

See [docs/ROADMAP.md](docs/ROADMAP.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
