---
title: 'Add per-agent job titles'
type: 'feature'
created: '2026-08-23'
status: 'done'
baseline_commit: '65954d64fa69a45733268f38dfc1e509afd80a05'
review_loop_iteration: 0
context: ['VISION.md', 'VISION_AGENT.md', 'TESTING.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Agents currently have a display name but no role/title, so a directory of named agents cannot communicate the business-like responsibility each agent owns.

**Approach:** Add an optional job title to the agent definition, make it editable in the existing agent settings dialog, persist and synchronize it with persona data, and show it beneath the agent name in the Agents directory and profile header.

## Boundaries & Constraints

**Always:** Existing agents and legacy persona events must deserialize with no title. Blank or whitespace-only titles are stored as unset. Definition edits remain the source of truth for linked instances. Existing card model/runtime fallback text remains unchanged when no title is set. Use the existing remount/query invalidation and UI styling patterns.

**Ask First:** None.

**Never:** Do not add a separate title database, change agent runtime behavior/prompts, repurpose the global Agent Defaults settings, or make title required.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| HAPPY_PATH | User enters `Principal Researcher` in the agent settings dialog and saves | The persona, its linked agent summaries, directory card, and profile header expose that title | Existing save error surface is used |
| CLEAR_TITLE | User removes the title and saves | Stored title is absent; the card falls back to its current model/runtime subtitle and the profile omits the title row | No error |
| LEGACY_DATA | Existing local record or inbound persona event has no title field | Title is `null`/unset and all existing behavior is preserved | No migration prompt or failure |
| LONG_OR_CONTROL_TEXT | Title contains leading/trailing whitespace or unsupported control characters | Trimmed visible text is validated using the existing agent text validation boundary | Return the existing validation error style |

</frozen-after-approval>

## Code Map

- `desktop/src/shared/api/types.ts` -- frontend persona/managed-agent input and response types.
- `desktop/src/shared/api/tauri.ts` and `desktop/src/shared/api/tauriPersonas.ts` -- snake_case/camelCase IPC mapping.
- `desktop/src/features/agents/ui/AgentDefinitionDialog.tsx` -- edit/create form and submit payload.
- `desktop/src/features/agents/ui/UnifiedAgentsSection.tsx` and `AgentIdentityCard.tsx` -- Agents directory presentation.
- `desktop/src/features/profile/ui/UserProfilePanel.tsx` and `UserProfilePanelSections.tsx` -- profile header presentation.
- `desktop/src-tauri/src/managed_agents/types.rs` -- persisted definition/record and summary types.
- `desktop/src-tauri/src/managed_agents/persona_events.rs` -- persona event wire content and projection.
- `desktop/src-tauri/src/commands/personas/{create,update,inbound}.rs` -- title normalization, save, and inbound merge.
- `desktop/src-tauri/src/managed_agents/agent_snapshot.rs` and `commands/personas/snapshot/import.rs` -- portable snapshot round-trip.

## Tasks & Acceptance

**Execution:**
- [x] Add optional `jobTitle`/`job_title` fields to the frontend and Rust definition, record, and summary models with legacy serde defaults.
- [x] Thread the field through persona create/update IPC, NIP-AP persona event serialization/parsing, inbound merge, and snapshot export/import.
- [x] Add an optional Job title input to `AgentDefinitionDialog`, trim/validate it, and preserve the existing blank-title fallback behavior.
- [x] Render the title on agent cards and the profile header, including a stable test id and no empty placeholder.
- [x] Add focused unit tests for event round-trip/legacy omission, normalization/clear behavior, and frontend fallback mapping.

**Acceptance Criteria:**
- Given an existing or new agent definition, when the user saves a non-empty Job title in agent settings, then reopening the settings shows the same title and the Agents directory displays it below the agent name.
- Given a linked deployed instance, when its definition title changes, then the directory/profile surfaces show the updated definition title without changing the agent prompt or runtime configuration.
- Given an empty title or legacy data, when the agent loads, then no title is rendered and the existing model/runtime subtitle remains visible.
- Given an inbound shared persona or portable snapshot containing a title, when it is imported, then the title round-trips; when it omits the field, then import remains backward-compatible.

## Spec Change Log

## Design Notes

The title is definition-level, like the existing display name and instructions. This keeps one title consistent across instances created from the same persona while preserving per-instance names from the existing name pool. The card uses the title as its secondary line when present; otherwise it retains the current model label.

## Verification

**Commands:**
- `cd desktop && pnpm exec biome check src` -- expected: no lint or formatting errors.
- `cd desktop && pnpm exec vitest run` -- expected: focused and existing desktop tests pass.
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml` -- expected: Tauri backend tests pass.

**Manual checks (if no CLI):**
- Open Agents, edit an agent, set a title, save, close/reopen its profile, and verify the title appears on the card and profile header; clear it and verify the prior fallback returns.

## Suggested Review Order

**Definition persistence and synchronization**

- The shared record fields preserve titles across persona projections and linked summaries.
  [`types.rs:16`](../../desktop/src-tauri/src/managed_agents/types.rs#L16)

- Create and update normalize titles while retaining explicit clear semantics for edits.
  [`update.rs:97`](../../desktop/src-tauri/src/commands/personas/update.rs#L97)

- Summary resolution honors an explicit persona clear before consulting stale materialized instance data.
  [`job_title.rs:3`](../../desktop/src-tauri/src/managed_agents/runtime/job_title.rs#L3)

- The persona event appends an optional field without changing legacy no-title payloads.
  [`persona_events.rs:48`](../../desktop/src-tauri/src/managed_agents/persona_events.rs#L48)

- Snapshot definitions carry the display metadata through portable export and import.
  [`agent_snapshot.rs:92`](../../desktop/src-tauri/src/managed_agents/agent_snapshot.rs#L92)

**Settings and presentation**

- The existing agent dialog exposes the optional title and sends a trimmed value.
  [`AgentDefinitionDialog.tsx:791`](../../desktop/src/features/agents/ui/AgentDefinitionDialog.tsx#L791)

- Cards prefer the configured title while preserving the existing model fallback.
  [`AgentIdentityCard.tsx:37`](../../desktop/src/features/agents/ui/AgentIdentityCard.tsx#L37)

- The profile hero renders a stable title row only when a title is present.
  [`UserProfilePanelSections.tsx:698`](../../desktop/src/features/profile/ui/UserProfilePanelSections.tsx#L698)

**Boundary validation and tests**

- Validation trims blank values and rejects oversized or invisible title text.
  [`definition_validation.rs:47`](../../desktop/src-tauri/src/managed_agents/definition_validation.rs#L47)

- Shared catalog parsing applies the same normalized, safe display-title boundary.
  [`personaCatalogRelay.ts:275`](../../desktop/src/features/agents/lib/personaCatalogRelay.ts#L275)

- Focused tests cover title round-trips, legacy compatibility, and UI/API mapping.
  [`persona_events/tests.rs:1`](../../desktop/src-tauri/src/managed_agents/persona_events/tests.rs#L1)
