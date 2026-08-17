import assert from "node:assert/strict";
import test from "node:test";

import { breadcrumbSegments, joinPath, parentPath } from "./fsPath.ts";

test("joinPath appends a child name under a directory", () => {
  assert.equal(joinPath("/workspace", "foo"), "/workspace/foo");
});

test("joinPath tolerates a trailing slash on the directory", () => {
  assert.equal(joinPath("/workspace/", "foo"), "/workspace/foo");
});

test("parentPath walks up one level", () => {
  assert.equal(
    parentPath("/workspace/foo/bar", "/workspace"),
    "/workspace/foo",
  );
});

test("parentPath clamps at the root", () => {
  assert.equal(parentPath("/workspace", "/workspace"), "/workspace");
});

test("parentPath does not rise above the root even if called past it", () => {
  assert.equal(
    parentPath("/workspace/foo", "/workspace/foo"),
    "/workspace/foo",
  );
});

test("breadcrumbSegments returns just the root label at the root", () => {
  assert.deepEqual(breadcrumbSegments("/workspace", "/workspace"), [
    { label: "workspace", path: "/workspace" },
  ]);
});

test("breadcrumbSegments builds one entry per path segment below root", () => {
  assert.deepEqual(breadcrumbSegments("/workspace/foo/bar", "/workspace"), [
    { label: "workspace", path: "/workspace" },
    { label: "foo", path: "/workspace/foo" },
    { label: "bar", path: "/workspace/foo/bar" },
  ]);
});

test("breadcrumbSegments handles the /home/agent root", () => {
  assert.deepEqual(breadcrumbSegments("/home/agent/x", "/home/agent"), [
    { label: "agent", path: "/home/agent" },
    { label: "x", path: "/home/agent/x" },
  ]);
});
