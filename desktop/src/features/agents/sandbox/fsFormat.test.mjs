import assert from "node:assert/strict";
import test from "node:test";

import { formatFileSize, formatRelativeMtime } from "./fsFormat.ts";

test("formatFileSize renders bytes under 1024 as-is", () => {
  assert.equal(formatFileSize(512), "512 B");
});

test("formatFileSize renders kilobytes with one decimal under 10", () => {
  assert.equal(formatFileSize(1536), "1.5 KB");
});

test("formatFileSize renders larger values with no decimal", () => {
  assert.equal(formatFileSize(1024 * 1024 * 42), "42 MB");
});

test("formatFileSize handles a negative or non-finite input", () => {
  assert.equal(formatFileSize(-1), "—");
  assert.equal(formatFileSize(Number.NaN), "—");
});

test("formatRelativeMtime renders 'just now' for very recent times", () => {
  const now = 1_000_000 * 1000;
  assert.equal(formatRelativeMtime(1_000_000 - 2, now), "just now");
});

test("formatRelativeMtime renders seconds, minutes, hours, days", () => {
  const now = 1_000_000 * 1000;
  assert.equal(formatRelativeMtime(1_000_000 - 30, now), "30s ago");
  assert.equal(formatRelativeMtime(1_000_000 - 120, now), "2m ago");
  assert.equal(formatRelativeMtime(1_000_000 - 7200, now), "2h ago");
  assert.equal(formatRelativeMtime(1_000_000 - 86400 * 3, now), "3d ago");
});

test("formatRelativeMtime never goes negative for a future mtime", () => {
  const now = 1_000_000 * 1000;
  assert.equal(formatRelativeMtime(1_000_100, now), "just now");
});
