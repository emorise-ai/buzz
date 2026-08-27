import assert from "node:assert/strict";
import test from "node:test";

import { containedAspectBox } from "./useContainedAspectBox.ts";

const RATIO_16_9 = 16 / 9;

test("width is the bottleneck: box shrinks to container width", () => {
  const box = containedAspectBox(800, 900, RATIO_16_9);
  assert.deepEqual(box, { width: 800, height: 450 });
});

test("height is the bottleneck: box shrinks to container height", () => {
  const box = containedAspectBox(1000, 300, RATIO_16_9);
  assert.equal(box.height, 300);
  assert.ok(Math.abs(box.width - 533.333) < 0.001);
});

test("an exact 16:9 container returns itself", () => {
  const box = containedAspectBox(1920, 1080, RATIO_16_9);
  assert.deepEqual(box, { width: 1920, height: 1080 });
});

test("returns null for a zero or negative container dimension", () => {
  assert.equal(containedAspectBox(0, 500, RATIO_16_9), null);
  assert.equal(containedAspectBox(500, 0, RATIO_16_9), null);
  assert.equal(containedAspectBox(-10, 500, RATIO_16_9), null);
});
