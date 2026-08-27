import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import { readComputerWindowHandoff } from "./computerWindow.ts";

// `readComputerWindowHandoff` only touches `window.location.hash`, so a
// minimal stand-in is enough — no need for the full Tauri/DOM environment
// the real pop-out window runs in.
beforeEach(() => {
  globalThis.window = { location: { hash: "" } };
});

test("no query string in the hash returns an all-null handoff", () => {
  window.location.hash = "#/computer/sbx-1";
  assert.deepEqual(readComputerWindowHandoff(), {
    viewerUrl: null,
    sandboxName: null,
    ownerPubkey: null,
    agentDisplayName: null,
    expiresAt: null,
  });
});

test("bare route with no hash at all returns an all-null handoff", () => {
  window.location.hash = "";
  assert.deepEqual(readComputerWindowHandoff(), {
    viewerUrl: null,
    sandboxName: null,
    ownerPubkey: null,
    agentDisplayName: null,
    expiresAt: null,
  });
});

test("round-trips every param the Rust side can send", () => {
  const viewerUrl = "https://relay.example.com/sandbox-viewer/sbx-1";
  window.location.hash =
    `#/computer/sbx-1?viewerUrl=${encodeURIComponent(viewerUrl)}` +
    `&sandboxName=${encodeURIComponent("buzz-sandbox-fq5ijxh7lt")}` +
    `&ownerPubkey=${"deadbeef".repeat(8)}` +
    `&agentDisplayName=${encodeURIComponent("Fern the Agent")}` +
    `&expiresAt=1700000000`;

  assert.deepEqual(readComputerWindowHandoff(), {
    viewerUrl,
    sandboxName: "buzz-sandbox-fq5ijxh7lt",
    ownerPubkey: "deadbeef".repeat(8),
    agentDisplayName: "Fern the Agent",
    expiresAt: 1_700_000_000,
  });
});

test("partial params fall back to null for whatever is missing", () => {
  window.location.hash =
    "#/computer/sbx-1?viewerUrl=https%3A%2F%2Fexample.com%2Fv";
  assert.deepEqual(readComputerWindowHandoff(), {
    viewerUrl: "https://example.com/v",
    sandboxName: null,
    ownerPubkey: null,
    agentDisplayName: null,
    expiresAt: null,
  });
});

test("a malformed expiresAt is dropped rather than producing NaN", () => {
  window.location.hash = "#/computer/sbx-1?expiresAt=not-a-number";
  assert.equal(readComputerWindowHandoff().expiresAt, null);
});
