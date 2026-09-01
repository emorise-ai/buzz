import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_SANDBOX_DESTROYED } from "@/shared/constants/kinds";
import { markComputerEverAssigned } from "./computerEverAssignedStore";
import { reconstructAgentSandbox, type AgentSandbox } from "./sandboxState";

/**
 * A single, community-wide subscription to sandbox lifecycle events, shared by
 * every agent card and profile that asks "does this agent have a computer?".
 *
 * Why a store and not a per-card hook: a per-card subscription meant one relay
 * REQ per rendered agent card *and* per profile panel — dozens of subscriptions
 * that tripped the relay's per-connection rate limit, so the events never
 * arrived (`rate-limited: quota exceeded`). One subscription for all agents,
 * fanned out by pubkey, stays well under quota. This mirrors the observer relay
 * store. Its reset MUST be wired into `resetCommunityState()`, or sandbox state
 * leaks across a community switch.
 */

// All seen lifecycle events, keyed by event id for dedup across replays.
let seenEvents = new Map<string, RelayEvent>();
// Reconstructed current sandbox per owner pubkey (lowercased). Absent = none.
let sandboxByOwner = new Map<string, AgentSandbox>();
// Immediate command-response state. This bridges the gap until the relay's
// lifecycle announcement arrives, and is essential when create reuses an
// already-running sandbox without publishing a fresh event.
let commandSandboxByOwner = new Map<string, AgentSandbox>();
// Successful DELETE is authoritative even when the broker says "already gone"
// and therefore has no destruction event to publish. Tombstones stop retained
// historical 48200 events from resurrecting that stale machine locally.
let locallyRemovedSandboxIds = new Set<string>();
const listeners = new Set<() => void>();
let unsubscribe: (() => void) | null = null;
let starting = false;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
let retryAttempt = 0;
// Bumped on reset so an in-flight subscribe callback from the old community is
// ignored once a new one begins.
let generation = 0;

const EMPTY: AgentSandbox | null = null;

function ownerTag(event: RelayEvent): string | null {
  return event.tags.find((t) => t[0] === "p")?.[1]?.toLowerCase() ?? null;
}

function notify() {
  for (const listener of listeners) listener();
}

/** Rebuild the per-owner map from the full event set. Cheap: sandbox events are
 *  rare. Reconstructs each owner independently so one agent's events never
 *  affect another's. */
function recompute() {
  const byOwner = new Map<string, RelayEvent[]>();
  const destroyedIds = new Set<string>();
  for (const event of seenEvents.values()) {
    if (event.kind === KIND_SANDBOX_DESTROYED) {
      const sandboxId = event.tags.find((tag) => tag[0] === "d")?.[1];
      if (sandboxId) destroyedIds.add(sandboxId);
    }
    const owner = ownerTag(event);
    if (!owner) continue;
    const list = byOwner.get(owner);
    if (list) list.push(event);
    else byOwner.set(owner, [event]);
  }

  const next = new Map<string, AgentSandbox>();
  for (const [owner, events] of byOwner) {
    const sandbox = reconstructAgentSandbox(events);
    if (
      sandbox &&
      !destroyedIds.has(sandbox.id) &&
      !locallyRemovedSandboxIds.has(sandbox.id)
    ) {
      next.set(owner, sandbox);
      // Passive observation: any owner seen with a live sandbox — not just
      // ones this client started — sticks in "ever had a computer" memory.
      markComputerEverAssigned(owner);
    }
  }
  for (const [owner, sandbox] of commandSandboxByOwner) {
    if (
      !destroyedIds.has(sandbox.id) &&
      !locallyRemovedSandboxIds.has(sandbox.id)
    ) {
      next.set(owner, sandbox);
    }
  }
  sandboxByOwner = next;
  notify();
}

/** Open the shared subscription once. Idempotent. */
async function ensureSubscription() {
  if (unsubscribe || starting || listeners.size === 0) return;
  starting = true;
  const gen = generation;
  let failed = false;
  try {
    const dispose = await relayClient.subscribeToAllSandboxEvents((event) => {
      if (gen !== generation) return;
      if (seenEvents.has(event.id)) return;
      seenEvents.set(event.id, event);
      const owner = ownerTag(event);
      const sandboxId = event.tags.find((tag) => tag[0] === "d")?.[1];
      if (owner && sandboxId) {
        const optimistic = commandSandboxByOwner.get(owner);
        if (optimistic?.id === sandboxId) commandSandboxByOwner.delete(owner);
      }
      recompute();
    });
    if (gen !== generation) {
      void dispose();
      return;
    }
    unsubscribe = () => void dispose();
    retryAttempt = 0;
  } catch (err) {
    console.error("[agentSandboxStore] subscription failed:", err);
    failed = true;
  } finally {
    starting = false;
    if (failed && gen === generation && listeners.size > 0 && !retryTimer) {
      const delayMs = Math.min(1_000 * 2 ** retryAttempt, 30_000);
      retryAttempt += 1;
      retryTimer = setTimeout(() => {
        retryTimer = null;
        void ensureSubscription();
      }, delayMs);
    }
  }
}

/** Apply a successful create/reuse response immediately. */
export function upsertSandboxForOwner(
  agentPubkey: string,
  sandbox: AgentSandbox,
): void {
  const owner = agentPubkey.toLowerCase();
  locallyRemovedSandboxIds.delete(sandbox.id);
  const current = sandboxByOwner.get(owner);
  const immediate =
    current?.id === sandbox.id
      ? {
          ...current,
          ...sandbox,
          viewerUrl: sandbox.viewerUrl ?? current.viewerUrl,
        }
      : sandbox;
  commandSandboxByOwner = new Map(commandSandboxByOwner).set(owner, immediate);
  markComputerEverAssigned(owner);
  recompute();
}

/** Apply a successful stop immediately, including broker "already gone". */
export function removeSandboxById(sandboxId: string): void {
  locallyRemovedSandboxIds = new Set(locallyRemovedSandboxIds).add(sandboxId);
  commandSandboxByOwner = new Map(
    [...commandSandboxByOwner].filter(
      ([, sandbox]) => sandbox.id !== sandboxId,
    ),
  );
  recompute();
}

/** Subscribe a React consumer to store changes. */
export function subscribeSandboxStore(listener: () => void): () => void {
  listeners.add(listener);
  void ensureSubscription();
  return () => {
    listeners.delete(listener);
  };
}

/** Current sandbox for one agent, or null. Stable reference between changes. */
export function getSandboxForOwner(
  agentPubkey: string | null,
): AgentSandbox | null {
  if (!agentPubkey) return EMPTY;
  return sandboxByOwner.get(agentPubkey.toLowerCase()) ?? EMPTY;
}

/**
 * Find a live sandbox by its own id, plus its owner's pubkey. Used by the
 * pop-out window route, which is opened with only a sandbox id (the window
 * label) and needs the owner's display name/profile too. A linear scan is
 * fine: an agent has at most one sandbox today, so this map is tiny.
 */
export function getSandboxById(
  sandboxId: string | null,
): { sandbox: AgentSandbox; ownerPubkey: string } | null {
  if (!sandboxId) return null;
  for (const [ownerPubkey, sandbox] of sandboxByOwner) {
    if (sandbox.id === sandboxId) return { sandbox, ownerPubkey };
  }
  return null;
}

/** Tear down on community switch. Wired into `resetCommunityState()`. */
export function resetAgentSandboxStore() {
  generation += 1;
  const dispose = unsubscribe;
  unsubscribe = null;
  starting = false;
  if (retryTimer) clearTimeout(retryTimer);
  retryTimer = null;
  retryAttempt = 0;
  seenEvents = new Map();
  sandboxByOwner = new Map();
  commandSandboxByOwner = new Map();
  locallyRemovedSandboxIds = new Set();
  notify();
  dispose?.();
}
