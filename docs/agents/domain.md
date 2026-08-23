# Domain docs

This repo uses a single-context layout.

- One `CONTEXT.md` at the repo root.
- ADRs live in `docs/adr/`.

### Reading CONTEXT.md

Treat `CONTEXT.md` as the single source of truth for the project's high-level context, decisions, and constraints. All ADRs referenced from it should be treated as authoritative.

### Reading ADRs

Each ADR in `docs/adr/` documents a decision context, options considered, and the resulting choice. When `CONTEXT.md` references an ADR by title or key, that ADR is considered binding for that decision area.