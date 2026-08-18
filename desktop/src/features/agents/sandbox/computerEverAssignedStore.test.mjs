import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  hasEverHadComputer,
  markComputerEverAssigned,
  resetComputerEverAssigned,
} from "./computerEverAssignedStore.ts";

// Real `Storage` shape (`.length` + `.key()`, not just get/set/remove) —
// `communityStorage.ts` and `localStorageQuota.ts` both walk existing keys on
// some paths, and a stub missing those throws mid-test in ways that are easy
// to miss locally but surface under a full-suite run. Match the shape the
// project's other localStorage tests already use (see
// `localStorageQuota.test.mjs`, `localStorageSweep.test.mjs`).
function localStorageStub() {
  const data = new Map();
  return {
    get length() {
      return data.size;
    },
    key: (i) => [...data.keys()][i] ?? null,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, String(value)),
    removeItem: (key) => data.delete(key),
  };
}

const ACTIVE_COMMUNITY_KEY = "buzz-active-community-id";

/**
 * Set the active community id directly on the stub's backing store, rather
 * than through `communityStorage.saveActiveCommunityId()`. Bypasses that
 * module's quota-recovery write path and `migrateLegacyCommunityStorage`'s
 * legacy-key migration entirely — this test only needs
 * `loadActiveCommunityId()` to read back whatever id was set, not to
 * exercise `communityStorage.ts`'s own behavior, which has its own test
 * coverage. Keeps this suite hermetic: no shared state, no dependency on
 * how another module resolves `localStorage` internally.
 */
function setActiveCommunityId(id) {
  window.localStorage.setItem(ACTIVE_COMMUNITY_KEY, id);
}

beforeEach(() => {
  const localStorage = localStorageStub();
  globalThis.window = { localStorage };
  // `communityStorage.ts`'s `migrateLegacyCommunityStorage` defaults its
  // `storage` param to the bare global `localStorage`, not `window.localStorage`
  // — set both so `loadActiveCommunityId()` resolves consistently regardless
  // of which one a given code path reads from.
  globalThis.localStorage = localStorage;
  setActiveCommunityId("community-a");
  resetComputerEverAssigned();
});

test("an agent never marked is not ever-had", () => {
  assert.equal(hasEverHadComputer("agent-a"), false);
});

test("marking an agent makes it ever-had", () => {
  markComputerEverAssigned("agent-a");
  assert.equal(hasEverHadComputer("agent-a"), true);
});

test("pubkey comparison is case-insensitive", () => {
  markComputerEverAssigned("AgentA");
  assert.equal(hasEverHadComputer("agenta"), true);
});

test("null pubkey is never ever-had", () => {
  assert.equal(hasEverHadComputer(null), false);
});

test("marking twice is idempotent", () => {
  markComputerEverAssigned("agent-a");
  markComputerEverAssigned("agent-a");
  assert.equal(hasEverHadComputer("agent-a"), true);
});

test("persists to localStorage under a community-scoped key", () => {
  markComputerEverAssigned("agent-a");
  const raw = window.localStorage.getItem(
    "buzz-computer-ever-assigned.v1:community-a",
  );
  assert.ok(raw);
  assert.deepEqual(JSON.parse(raw), ["agent-a"]);
});

test("reset clears in-memory state but storage survives for reload", () => {
  markComputerEverAssigned("agent-a");
  resetComputerEverAssigned();
  // Same community on next touch — reloads from storage, so it's still true.
  assert.equal(hasEverHadComputer("agent-a"), true);
});

test("different community ids read/write separate sets", () => {
  markComputerEverAssigned("agent-a");
  assert.equal(hasEverHadComputer("agent-a"), true);

  setActiveCommunityId("community-b");
  resetComputerEverAssigned();
  assert.equal(hasEverHadComputer("agent-a"), false);

  markComputerEverAssigned("agent-b");
  assert.equal(hasEverHadComputer("agent-b"), true);

  setActiveCommunityId("community-a");
  resetComputerEverAssigned();
  assert.equal(hasEverHadComputer("agent-a"), true);
  assert.equal(hasEverHadComputer("agent-b"), false);
});

test("a throwing loadActiveCommunityId does not crash reads or writes", () => {
  // Simulate the WebKit denied-storage SecurityError path: getItem throws.
  globalThis.window = {
    localStorage: {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("SecurityError");
      },
      removeItem: () => {},
      length: 0,
      key: () => null,
    },
  };
  globalThis.localStorage = globalThis.window.localStorage;
  resetComputerEverAssigned();
  assert.doesNotThrow(() => markComputerEverAssigned("agent-a"));
  assert.doesNotThrow(() => hasEverHadComputer("agent-a"));
});
