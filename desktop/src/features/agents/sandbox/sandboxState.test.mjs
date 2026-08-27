import assert from "node:assert/strict";
import test from "node:test";

import { reconstructAgentSandbox } from "./sandboxState.ts";

const OWNER = "a".repeat(64);

function created(id, overrides = {}, createdAt = 1000) {
  const tags = [
    ["d", id],
    ["name", overrides.name ?? `buzz-sandbox-${id}`],
    ["image", overrides.image ?? "sprig-desktop"],
    ["cpus", String(overrides.cpus ?? 2)],
    ["memory_mb", String(overrides.memoryMb ?? 4096)],
    ["expires_at", String(overrides.expiresAt ?? 9_000)],
    ["p", OWNER],
  ];
  if (overrides.viewer !== undefined) tags.push(["viewer", overrides.viewer]);
  return {
    id: `evt-created-${id}-${createdAt}`,
    kind: 48200,
    pubkey: "broker",
    created_at: createdAt,
    content: "",
    tags,
  };
}

function destroyed(id, reason = "expired", createdAt = 2000) {
  return {
    id: `evt-destroyed-${id}-${createdAt}`,
    kind: 48201,
    pubkey: "broker",
    created_at: createdAt,
    content: "",
    tags: [
      ["d", id],
      ["reason", reason],
      ["p", OWNER],
    ],
  };
}

test("no events means no sandbox", () => {
  assert.equal(reconstructAgentSandbox([]), null);
});

test("a lone created event yields a live sandbox with parsed budget", () => {
  const sandbox = reconstructAgentSandbox([
    created("s1", { cpus: 4, memoryMb: 8192, viewer: "https://v/s1/" }),
  ]);
  assert.ok(sandbox);
  assert.equal(sandbox.id, "s1");
  assert.equal(sandbox.cpus, 4);
  assert.equal(sandbox.memoryMb, 8192);
  assert.equal(sandbox.viewerUrl, "https://v/s1/");
  assert.equal(sandbox.image, "sprig-desktop");
});

test("a matching destroyed event removes the sandbox", () => {
  assert.equal(reconstructAgentSandbox([created("s1"), destroyed("s1")]), null);
});

test("order does not matter — destroyed before created still removes it", () => {
  assert.equal(reconstructAgentSandbox([destroyed("s1"), created("s1")]), null);
});

test("a destroyed event for a different sandbox does not remove the live one", () => {
  const sandbox = reconstructAgentSandbox([created("s1"), destroyed("s2")]);
  assert.ok(sandbox);
  assert.equal(sandbox.id, "s1");
});

test("the newest live sandbox wins when several are live", () => {
  const sandbox = reconstructAgentSandbox([
    created("old", {}, 1000),
    created("new", {}, 5000),
  ]);
  assert.equal(sandbox.id, "new");
});

test("a re-announcement (later 48200, same id) replaces the earlier facts", () => {
  const sandbox = reconstructAgentSandbox([
    created("s1", { viewer: undefined }, 1000),
    created("s1", { viewer: "https://v/s1/" }, 2000),
  ]);
  assert.equal(sandbox.viewerUrl, "https://v/s1/");
});

test("missing viewer tag yields a null viewerUrl", () => {
  const sandbox = reconstructAgentSandbox([created("s1")]);
  assert.equal(sandbox.viewerUrl, null);
});

test("a malformed numeric tag becomes null rather than NaN", () => {
  const event = created("s1");
  event.tags = event.tags.map((t) => (t[0] === "cpus" ? ["cpus", "lots"] : t));
  const sandbox = reconstructAgentSandbox([event]);
  assert.equal(sandbox.cpus, null);
});

test("an event with no d tag is ignored", () => {
  const event = created("s1");
  event.tags = event.tags.filter((t) => t[0] !== "d");
  assert.equal(reconstructAgentSandbox([event]), null);
});
