import assert from "node:assert/strict";
import test from "node:test";

import { shouldSendHeartbeat } from "./sandboxHeartbeat.ts";

test("pings while active, unexpired, and visible", () => {
  assert.equal(shouldSendHeartbeat(true, false, "visible"), true);
});

test("skips when the stage isn't active (unmounted/backgrounded surface)", () => {
  assert.equal(shouldSendHeartbeat(false, false, "visible"), false);
});

test("skips once the sandbox is expired", () => {
  assert.equal(shouldSendHeartbeat(true, true, "visible"), false);
});

test("skips while the tab/window is hidden, even if active", () => {
  assert.equal(shouldSendHeartbeat(true, false, "hidden"), false);
});

test("expired and hidden both losing still reads as skip, not a crash", () => {
  assert.equal(shouldSendHeartbeat(false, true, "hidden"), false);
});
