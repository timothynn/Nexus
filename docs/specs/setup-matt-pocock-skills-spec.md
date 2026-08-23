## Problem Statement

A new Nexus repo initialization for engineering skills. When a new or existing Nexus repo lacks the per-repo configuration that engineering skills assume, skills like `to-tickets`, `triage`, `to-spec`, and others cannot determine where issues are tracked, what triage labels to use, or what domain doc layout to follow. This creates ambiguity and prevents these skills from functioning correctly out of the box.

The root cause is that each repo needs explicit configuration for: (1) where issues are tracked (GitHub, GitLab, local markdown, or other), (2) the vocabulary of triage label strings, and (3) the domain doc layout convention (single-context or multi-context). Without this configuration, skills default to behaviors that may not match the repo's actual workflow.

## Solution

Scaffold the per-repo configuration that engineering skills assume by creating the standard documentation files and updating the project's AGENTS.md. This involves:

1. Creating `docs/agents/issue-tracker.md` - documenting the issue tracking location and workflow
2. Creating `docs/agents/triage-labels.md` - defining the canonical triage label strings
3. Creating `docs/agents/domain.md` - specifying the domain doc layout convention
4. Updating `AGENTS.md` with a `## Agent skills` block referencing these docs

The configuration enables engineering skills to read from these files and operate consistently across repos that follow the same conventions.

## User Stories

1. As a new developer joining a Nexus repo, I want the repo to have standard issue tracker documentation so that skills like `to-tickets` can locate where work items are tracked.

2. As a triage moderator, I want the repo to have defined triage labels so that the `triage` skill can route issues using consistent label strings.

3. As an agent designer, I want the repo to have a documented domain doc layout so that `CONTEXT.md` and ADRs follow a predictable structure.

4. As a repo maintainer, I want to update the issue tracker or triage labels without breaking engineering skills, by editing the documented files rather than changing skill behavior.

5. As an onboarding agent, I want to run `/setup-matt-pocock-skills` once and have all necessary documentation files created with correct content.

## Implementation Decisions

- **Issue tracker**: Default to GitHub Issues when the repo has a GitHub remote, as was the case for this repo (origin: https://github.com/timothynn/Nexus.git). The configuration carries a "PRs as a request surface" flag defaulted off.

- **Triage labels**: Use the five canonical role names as label strings: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. These are the defaults that the `triage` skill expects.

- **Domain docs**: Adopt single-context layout (one `CONTEXT.md` at root + `docs/adr/` for ADRs), which fits almost every repo and was the default choice for this project.

- **AGENTS.md update**: Add `## Agent skills` block referencing the three doc files. This block is appended after the existing `## Documentation` section.

- **File placement**: All docs live under `docs/agents/` at the repo root, consistent with the single-context layout convention.

- **Skill invocation**: The `/setup-matt-pocock-skills` skill handles the full setup process, but the individual files can also be created or edited manually.

## Testing Decisions

- The setup is verified by confirming: (a) `docs/agents/` directory exists with all three markdown files, (b) `AGENTS.md` contains the `## Agent skills` block with correct references, (c) git remote points to GitHub (for default issue tracker behavior).

- Existing tests in the workspace (`cargo test --workspace`) should continue to pass after the setup changes, as no code logic is modified - only documentation files are created.

- The `ready-for-agent` triage label is applied to the spec itself, as requested.

## Out of Scope

- Support for issue trackers other than GitHub, GitLab, and local markdown (these would require custom `/setup-matt-pocock-skills` invocation with different parameters).
- Changing the underlying skill logic - only the documentation files are created/modified.
- Creating ADRs or `CONTEXT.md` at the repo root (these are separate from the skills setup, though the domain doc layout convention acknowledges their existence).
- Multi-context layout unless the repo has monorepo signals (which this repo does not).

## Further Notes

- The three documentation files (`issue-tracker.md`, `triage-labels.md`, `domain.md`) under `docs/agents/` are the minimal set needed for engineering skills to function.
- Repos can customize any of these files after initial setup - the skills read from them at runtime, so changes take effect immediately.
- The `## Agent skills` block in `AGENTS.md` should be kept in sync with the actual doc files - if the doc files are modified, the block summary may need updating.
- This setup is idempotent - running `/setup-matt-pocock-skills` again on an already-setup repo will overwrite the files but produce the same result.
- The `ready-for-agent` triage label applied to this spec indicates it's ready for an agent to implement the described setup process.