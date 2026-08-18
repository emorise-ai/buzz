import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
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
const listeners = new Set<() => void>();
let unsubscribe: (() => void) | null = null;
let starting = false;
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
  for (const event of seenEvents.values()) {
    const owner = ownerTag(event);
    if (!owner) continue;
    const list = byOwner.get(owner);
    if (list) list.push(event);
    else byOwner.set(owner, [event]);
  }

  const next = new Map<string, AgentSandbox>();
  for (const [owner, events] of byOwner) {
    const sandbox = reconstructAgentSandbox(events);
    if (sandbox) next.set(owner, sandbox);
  }
  sandboxByOwner = next;
  console.error(
    "[agentSandboxStore] recompute — owners with a computer:",
    [...next.keys()].map((k) => k.slice(0, 8)),
  );
  notify();
}

/** Open the shared subscription once. Idempotent. */
async function ensureSubscription() {
  if (unsubscribe || starting) return;
  starting = true;
  const gen = generation;
  console.error("[agentSandboxStore] ensureSubscription: opening");

  try {
    const dispose = await relayClient.subscribeToAllSandboxEvents((event) => {
      console.error("[agentSandboxStore] EVENT kind", event.kind);
      if (gen !== generation) return;
      if (seenEvents.has(event.id)) return;
      seenEvents.set(event.id, event);
      recompute();
    });
    console.error("[agentSandboxStore] subscribed OK");
    if (gen !== generation) {
      void dispose();
      return;
    }
    unsubscribe = () => void dispose();
  } catch (err) {
    console.error("[agentSandboxStore] subscription failed:", err);
  } finally {
    starting = false;
  }
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
  seenEvents = new Map();
  sandboxByOwner = new Map();
  notify();
  dispose?.();
}
