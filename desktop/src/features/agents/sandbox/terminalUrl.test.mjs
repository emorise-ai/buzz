import assert from "node:assert/strict";
import test from "node:test";

import { deriveTerminalUrl } from "./terminalUrl.ts";

test("swaps a trailing /desktop for /terminal", () => {
  assert.equal(
    deriveTerminalUrl("https://relay.example.com/sandbox-viewer/s1/desktop"),
    "https://relay.example.com/sandbox-viewer/s1/terminal",
  );
});

test("returns null when the viewer URL does not end in /desktop", () => {
  assert.equal(
    deriveTerminalUrl("https://relay.example.com/sandbox-viewer/s1/screen"),
    null,
  );
});

test("only replaces a trailing /desktop, not one in the middle of the path", () => {
  assert.equal(
    deriveTerminalUrl("https://relay.example.com/desktop/sandbox-viewer/s1"),
    null,
  );
});

test("derives from an already-minted viewer URL by stripping the token query", () => {
  assert.equal(
    deriveTerminalUrl(
      "https://x.example/sandbox-viewer/sandboxes/abc123/desktop?t=sometoken",
    ),
    "https://x.example/sandbox-viewer/sandboxes/abc123/terminal",
  );
});
