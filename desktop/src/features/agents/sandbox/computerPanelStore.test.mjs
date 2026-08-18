import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  closeComputerPanel,
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
  });
  closeComputerPanel();
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: false,
    ownerPubkey: null,
  });
});

test("closing an already-closed panel is a no-op", () => {
  closeComputerPanel();
  assert.deepEqual(getComputerPanelSnapshotForTests(), {
    open: false,
    ownerPubkey: null,
  });
});

test("toggle opens, then closes for the same owner", () => {
  toggleComputerPanel("agent-a");
  assert.equal(getComputerPanelSnapshotForTests().open, true);
  assert.equal(getComputerPanelSnapshotForTests().ownerPubkey, "agent-a");
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
  });
});
