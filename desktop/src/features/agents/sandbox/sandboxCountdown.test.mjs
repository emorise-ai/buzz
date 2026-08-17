import assert from "node:assert/strict";
import test from "node:test";

import {
  formatSandboxRemaining,
  isSandboxExpired,
} from "./sandboxCountdown.ts";

const now = 1_000_000_000_000; // fixed "now" in ms
const nowSec = now / 1000;

test("seconds granularity in the final minute", () => {
  assert.equal(formatSandboxRemaining(nowSec + 45, now), "45s left");
});

test("minutes granularity under an hour", () => {
  assert.equal(formatSandboxRemaining(nowSec + 25 * 60, now), "25m left");
});

test("hours and minutes over an hour", () => {
  assert.equal(
    formatSandboxRemaining(nowSec + 2 * 3600 + 15 * 60, now),
    "2h 15m left",
  );
});

test("whole hours drop the minutes", () => {
  assert.equal(formatSandboxRemaining(nowSec + 3 * 3600, now), "3h left");
});

test("a passed deadline reads expired, never negative", () => {
  assert.equal(formatSandboxRemaining(nowSec - 10, now), "expired");
  assert.equal(formatSandboxRemaining(nowSec, now), "expired");
});

test("isSandboxExpired flips at the deadline", () => {
  assert.equal(isSandboxExpired(nowSec + 1, now), false);
  assert.equal(isSandboxExpired(nowSec, now), true);
  assert.equal(isSandboxExpired(nowSec - 1, now), true);
});
