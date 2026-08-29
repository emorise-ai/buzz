import * as React from "react";

/**
 * Sidebar computer-preview panel toggle, mirroring `terminalPanelStore.ts`.
 * A store (not a prop threaded through `ChannelScreen.tsx` → `ChannelPane`)
 * because the panel's only state is "open, for this owner pubkey, or
 * closed" — the header button and the aux-panel branch in `ChannelPane` can
 * both read it directly without adding a new prop to every layer between
 * them, the same tradeoff the terminal panel already made.
 */

type Snapshot = {
  open: boolean;
  /** The agent whose computer is showing, or null when closed. */
  ownerPubkey: string | null;
  /** One-shot request to open the large viewer as soon as the screen is ready. */
  viewerRequested: boolean;
};

let snapshot: Snapshot = {
  open: false,
  ownerPubkey: null,
  viewerRequested: false,
};
const listeners = new Set<() => void>();

function publish(next: Snapshot) {
  snapshot = next;
  for (const listener of listeners) listener();
}

export function openComputerPanel(ownerPubkey: string) {
  publish({ open: true, ownerPubkey, viewerRequested: true });
}

export function closeComputerPanel() {
  if (!snapshot.open) return;
  publish({ open: false, ownerPubkey: null, viewerRequested: false });
}

/** Mark the large-view request handled without closing the docked screen. */
export function consumeComputerViewerRequest(ownerPubkey: string) {
  if (
    !snapshot.open ||
    snapshot.ownerPubkey !== ownerPubkey ||
    !snapshot.viewerRequested
  ) {
    return;
  }
  publish({ ...snapshot, viewerRequested: false });
}

/** Toggle the panel for `ownerPubkey`: closes if already open for this
 *  owner, otherwise opens (switching owners if a different one was open). */
export function toggleComputerPanel(ownerPubkey: string) {
  if (snapshot.open && snapshot.ownerPubkey === ownerPubkey) {
    closeComputerPanel();
  } else {
    openComputerPanel(ownerPubkey);
  }
}

export function useComputerPanel() {
  return React.useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => snapshot,
  );
}

export function resetComputerPanelForTests() {
  snapshot = { open: false, ownerPubkey: null, viewerRequested: false };
}

export function getComputerPanelSnapshotForTests() {
  return snapshot;
}
