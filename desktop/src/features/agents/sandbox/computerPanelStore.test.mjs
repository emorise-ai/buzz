import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  closeComputerPanel,
  consumeComputerViewerRequest,
  getComputerPanelSnapshotForTests,
  openComputerPanel,
  resetComputerPanelForTests,
  toggleComputerPanel,
} from "./computerPanelStore.ts";

beforeEach(resetComputerPanelForTests);

test("panel opens for an owner and closes", () => {
  openComputerPanel("agent-a");
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: true,
    ownerPubkey: "agent-a",
    viewerRequested: true,
  });
  closeComputerPanel();
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: false,
    ownerPubkey: null,
    viewerRequested: false,
  });
});

test("closing an already-closed panel is a no-op", () => {
  closeComputerPanel();
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: false,
    ownerPubkey: null,
    viewerRequested: false,
  });
});

test("toggle opens, then closes for the same owner", () => {
  toggleComputerPanel("agent-a");
  assert.equal(getComputerPanelSnapshotForTests().open, true);
  assert.equal(getComputerPanelSnapshotForTests().ownerPubkey, "agent-a");
  assert.equal(getComputerPanelSnapshotForTests().viewerRequested, true);
  toggleComputerPanel("agent-a");
  assert.equal(getComputerPanelSnapshotForTests().open, false);
});

test("toggle for a different owner switches rather than closing", () => {
  // Clicking the Computer button in another DM while the panel is already
  // open for a different agent should switch to that agent's computer, not
  // close the panel — the button always means "show/hide this DM's agent."
  toggleComputerPanel("agent-a");
  toggleComputerPanel("agent-b");
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: true,
    ownerPubkey: "agent-b",
    viewerRequested: true,
  });
});

test("viewer request is consumed without closing the panel", () => {
  openComputerPanel("agent-a");
  consumeComputerViewerRequest("agent-a");
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: true,
    ownerPubkey: "agent-a",
    viewerRequested: false,
  });
});

test("another owner cannot consume the viewer request", () => {
  openComputerPanel("agent-a");
  consumeComputerViewerRequest("agent-b");
  assert.equal(getComputerPanelSnapshotForTests().viewerRequested, true);
});
