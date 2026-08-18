import * as React from "react";

import { loadActiveCommunityId } from "@/features/communities/communityStorage";

/**
 * "Has this agent ever had a computer?" — sticky per-agent memory so the
 * Computer affordance (header button, message-list indicator) stays visible
 * once a sandbox has existed, even after it expires or is stopped. Without
 * this, the icon would vanish the moment a sandbox is destroyed and an agent
 * with a history of computer use would look indistinguishable from one that
 * never had one.
 *
 * Community-scoped, mirroring `channelSectionsStorage.storageKey(pubkey,
 * relayUrl)`: the localStorage key carries the active community id so a
 * pubkey seen "ever assigned" in one community doesn't leak the affordance
 * into another. Unlike that store, this one is keyed by community id alone
 * (no pubkey) — resolved synchronously via `loadActiveCommunityId()`, which
 * every module-level caller (agentSandboxStore's recompute loop,
 * sandboxLifecycle's createAgentSandbox) can reach without threading props.
 *
 * In-memory `Set` mirrors `computerPanelStore.ts`'s useSyncExternalStore
 * shape; localStorage is the durable backing so the dim "asleep" icon
 * survives an app restart, not just the current session.
 */

const STORAGE_KEY_PREFIX = "buzz-computer-ever-assigned.v1";

function storageKey(communityId: string): string {
  return `${STORAGE_KEY_PREFIX}:${communityId}`;
}

let everAssigned = new Set<string>();
let loadedForCommunityId: string | null = null;
const listeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) listener();
}

function readFromStorage(communityId: string): Set<string> {
  try {
    const raw = window.localStorage.getItem(storageKey(communityId));
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((v): v is string => typeof v === "string"));
  } catch {
    return new Set();
  }
}

function writeToStorage(communityId: string, set: Set<string>): void {
  try {
    window.localStorage.setItem(
      storageKey(communityId),
      JSON.stringify([...set]),
    );
  } catch {
    // Best-effort — worst case the dim icon doesn't survive a restart.
  }
}

/** Ensure the in-memory set reflects the currently active community, lazily
 *  loading from storage on first touch (or after a community switch cleared
 *  it, since `resetComputerEverAssigned` nulls `loadedForCommunityId`).
 *
 *  `loadActiveCommunityId()` is defensive on its own (fails closed to `null`
 *  on a denied-storage SecurityError — see its docstring), but it's still an
 *  external call from a module-level store with no error boundary above it,
 *  so a wrapping try/catch keeps a surprise throw there from taking down
 *  every "does this agent have a computer" read in the app. */
function ensureLoaded(): string | null {
  let communityId: string | null = null;
  try {
    communityId = loadActiveCommunityId();
  } catch {
    communityId = null;
  }
  if (communityId && communityId !== loadedForCommunityId) {
    everAssigned = readFromStorage(communityId);
    loadedForCommunityId = communityId;
  }
  return communityId;
}

/** Record that `pubkey` has (or has had) a computer. Called both from the
 *  passive relay-observation loop (any agent seen with a live sandbox) and
 *  right after a successful `createAgentSandbox()`. */
export function markComputerEverAssigned(pubkey: string): void {
  const communityId = ensureLoaded();
  const normalized = pubkey.toLowerCase();
  if (everAssigned.has(normalized)) return;
  everAssigned = new Set(everAssigned).add(normalized);
  if (communityId) writeToStorage(communityId, everAssigned);
  notify();
}

export function hasEverHadComputer(pubkey: string | null): boolean {
  if (!pubkey) return false;
  ensureLoaded();
  return everAssigned.has(pubkey.toLowerCase());
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useHasEverHadComputer(pubkey: string | null): boolean {
  const getSnapshot = React.useCallback(
    () => hasEverHadComputer(pubkey),
    [pubkey],
  );
  return React.useSyncExternalStore(subscribe, getSnapshot);
}

/** Tear down on community switch. Wired into `resetCommunityState()`. The
 *  localStorage key is already community-scoped, so this only needs to drop
 *  the in-memory cache — the next `ensureLoaded()` call re-reads under the
 *  new community's key. */
export function resetComputerEverAssigned(): void {
  everAssigned = new Set();
  loadedForCommunityId = null;
  notify();
}
