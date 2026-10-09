# ADR 0002: A text-embedded action protocol for bots

- **Status:** Accepted
- **Area:** `nexus-teams` (`protocol.rs`, `runtime_brain.rs`)

## Context

A bot's turn has to produce both conversation (what it says to the team) and structured steps: creating and updating tasks, DMs, handoffs, votes, and completion proposals. Bots can be backed by any provider. Some support native tool calling and some don't, and offline brains (simulated, scripted) must speak the same language.

Model-backed bots may also call workspace tools (`filesystem.read`, `shell.execute`) during a turn. Those go through the agent runtime's tool loop and permission checks.

## Options considered

1. **Native tool calling for team actions.** This gives schema validation by the provider. But it only works with tool-capable models, mixes collaboration with workspace tools in one namespace, and splits a reply into several provider-specific shapes.
2. **JSON-only replies.** This is easy to parse. But it is brittle (one stray token fails the whole turn), and it hides the conversational text that makes the transcript readable.
3. **Prose plus one fenced `nexus-actions` JSON block.** The prose is posted as a chat message, and the optional block carries a JSON array of typed actions.

## Decision

Bots reply in prose with at most one fenced block tagged `nexus-actions` containing a JSON array of `BotAction`s (`{"type": "create_task", …}`), parsed by `parse_reply`.

- Prose outside the block becomes a `post` to the channel where the bot was addressed.
- A malformed block does not fail the turn. The prose is still posted, and the parse problem is reported as `protocol.warning` activity, so the bot sees its own mistake in later context.
- Task ids are accepted as `3`, `"3"`, or `"T3"`, and optional fields have defaults, to tolerate model variation.
- `protocol_guide()` is the single description of the protocol given to every model-backed bot. Simulated and scripted brains produce `BotAction`s directly.
- Native tool calling stays reserved for workspace tools inside a turn, executed by `AgentRuntime` through the permissioned executor.

## Consequences

- Any chat model can be a team member, including models without tool calling. The transcript stays human-readable.
- Validation happens in Nexus, not in the provider. The engine still checks every action: permissions, membership, dependency gates, and proposal state.
- Adding an action means a new `BotAction` variant, a `team.*` permission string, an engine handler, and a line in `protocol_guide()`.
