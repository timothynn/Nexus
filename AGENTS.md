# Nexus Agent Instructions

## Development loop

1. Read the affected crate boundaries before editing.
2. Implement one vertical slice at a time at stable public seams.
3. Add or update focused tests with each behavior change.
4. Run formatting, build, tests, and Clippy before declaring a change complete.
5. Keep provider, UI, storage, workspace, and tool implementations behind explicit contracts.

## Architecture rules

- `nexus-core` stays dependency-light.
- The runtime coordinates execution; it should not own provider-specific HTTP or workspace-specific Git details.
- Tool execution must pass through permissions.
- Agent changes must never be merged automatically into the human checkout.
- Auditability and cancellation are part of the execution contract, not optional logging.
- New integrations should be additive and isolated behind dedicated crates or adapters.

## Documentation

Update `README.md` and `docs/ROADMAP.md` whenever implemented status or CLI surfaces change.

## Agent skills

### Issue tracker

This repo uses GitHub Issues to track work and bugs. See `docs/agents/issue-tracker.md`.

### Triage labels

The following label strings are used for the five canonical triage roles: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

This repo uses a single-context layout. See `docs/agents/domain.md`.
