import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

const storage = new Map();
globalThis.window = {
  localStorage: {
    getItem: (key) => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, value),
    removeItem: (key) => storage.delete(key),
  },
};
globalThis.localStorage = globalThis.window.localStorage;

const {
  getSandboxForOwner,
  removeSandboxById,
  resetAgentSandboxStore,
  upsertSandboxForOwner,
} = await import("./agentSandboxStore.ts");

const OWNER = "a".repeat(64);
const SANDBOX = {
  id: "sandbox-1",
  name: "buzz-sandbox-1",
  image: "buzz-sprig-desktop",
  cpus: 2,
  memoryMb: 8192,
  expiresAt: 12_345,
  viewerUrl: "https://relay.example/sandbox-viewer/sandbox-1",
  createdAt: 100,
};

afterEach(() => {
  resetAgentSandboxStore();
  storage.clear();
});

test("successful create is visible without waiting for a relay event", () => {
  upsertSandboxForOwner(OWNER, SANDBOX);
  assert.deepEqual(getSandboxForOwner(OWNER), SANDBOX);
});

test("successful idempotent stop clears stale state immediately", () => {
  upsertSandboxForOwner(OWNER, SANDBOX);
  removeSandboxById(SANDBOX.id);
  assert.equal(getSandboxForOwner(OWNER), null);
});
