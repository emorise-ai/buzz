import { expect, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { waitForAnimations } from "../helpers/animations";

const SHOTS = "test-results/sandbox-preview";

// A running managed agent that owns the sandbox. Tyler's pubkey renders as a
// managed-agent row in the running section.
const AGENT_PUBKEY = TEST_IDENTITIES.tyler.pubkey;

// A self-contained data: URL that stands in for the noVNC viewer, so the full
// view renders a recognizable "screen" without a live sandbox or network.
const MOCK_VIEWER = `data:text/html,${encodeURIComponent(
  `<html><body style="margin:0;background:#101418;color:#cdd6f4;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh"><div style="text-align:center"><div style="font-size:48px">🖥️</div><div>Agent desktop (noVNC)</div></div></body></html>`,
)}`;

/** Emit a sandbox-created (48200) event for the agent and wait for the card. */
async function emitSandbox(
  page: import("@playwright/test").Page,
  overrides: Record<string, unknown> = {},
) {
  // The card opens the owner-scoped subscription on mount; give the REQ a beat
  // to register before emitting, or the live event is dropped.
  await page.waitForTimeout(300);
  await page.evaluate(
    ({ ownerPubkey, extra }) => {
      const emit = (
        window as Window & {
          __BUZZ_E2E_EMIT_MOCK_SANDBOX__?: (input: unknown) => unknown;
        }
      ).__BUZZ_E2E_EMIT_MOCK_SANDBOX__;
      if (!emit) throw new Error("sandbox emit hook unavailable");
      emit({ ownerPubkey, ...(extra as Record<string, unknown>) });
    },
    { ownerPubkey: AGENT_PUBKEY, extra: overrides },
  );
}

// Open the agent's profile panel — where the sandbox preview lives — by
// clicking its card in the agents grid.
async function gotoAgentProfile(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId("open-agents-view").click();
  const card = page.getByRole("button", { name: "Tyler Agent agent profile" });
  await expect(card).toBeVisible({ timeout: 10_000 });
  await card.click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible({
    timeout: 10_000,
  });
}

test.describe("agent sandbox preview screenshots", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test.beforeEach(async ({ page }) => {
    await installMockBridge(page, {
      managedAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Tyler Agent",
          status: "running" as const,
          channelNames: ["agents"],
        },
      ],
    });
  });

  // Shot 01: the preview card on the agent — "has a computer", budget, countdown.
  test("01-preview-card", async ({ page }) => {
    await gotoAgentProfile(page);
    await emitSandbox(page, {
      viewerUrl: MOCK_VIEWER,
      expiresAt: Math.floor(Date.now() / 1000) + 42 * 60,
    });

    const preview = page.getByTestId(`agent-sandbox-preview-${AGENT_PUBKEY}`);
    await expect(preview).toBeVisible({ timeout: 10_000 });
    await expect(preview).toContainText("Has a computer");
    await expect(preview).toContainText("sprig-desktop");
    await expect(preview).toContainText("left");
    await waitForAnimations(page);
    await preview.screenshot({ path: `${SHOTS}/01-preview-card.png` });
  });

  // Shot 02: clicking the preview opens the full screen view with the
  // human-in-control note.
  test("02-full-view", async ({ page }) => {
    await gotoAgentProfile(page);
    await emitSandbox(page, {
      viewerUrl: MOCK_VIEWER,
      expiresAt: Math.floor(Date.now() / 1000) + 42 * 60,
    });

    const preview = page.getByTestId(`agent-sandbox-preview-${AGENT_PUBKEY}`);
    await expect(preview).toBeVisible({ timeout: 10_000 });
    await preview.click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("share this screen with the agent");
    await expect(dialog.locator("iframe")).toBeVisible();
    await waitForAnimations(page);
    await dialog.screenshot({ path: `${SHOTS}/02-full-view.png` });
  });

  // Shot 03: a sandbox whose screen is not yet routed shows the budget but is
  // not clickable — the pre-viewer state on staging today.
  test("03-no-viewer", async ({ page }) => {
    await gotoAgentProfile(page);
    await emitSandbox(page, {
      expiresAt: Math.floor(Date.now() / 1000) + 42 * 60,
    });

    const preview = page.getByTestId(`agent-sandbox-preview-${AGENT_PUBKEY}`);
    await expect(preview).toBeVisible({ timeout: 10_000 });
    await expect(preview).toContainText("Screen not reachable");
    await expect(preview).toBeDisabled();
    await waitForAnimations(page);
    await preview.screenshot({ path: `${SHOTS}/03-no-viewer.png` });
  });

  // Shot 05: the grid card shows a "has a computer" glyph so the sandbox is
  // discoverable without opening the panel.
  test("05-card-indicator", async ({ page }) => {
    await page.goto("/");
    await page.getByTestId("open-agents-view").click();
    const card = page.getByRole("button", {
      name: "Tyler Agent agent profile",
    });
    await expect(card).toBeVisible({ timeout: 10_000 });

    await page.waitForTimeout(300);
    await page.evaluate((ownerPubkey) => {
      const emit = (
        window as Window & {
          __BUZZ_E2E_EMIT_MOCK_SANDBOX__?: (input: unknown) => unknown;
        }
      ).__BUZZ_E2E_EMIT_MOCK_SANDBOX__;
      emit?.({ ownerPubkey });
    }, AGENT_PUBKEY);

    const indicator = page.getByTestId(
      `agent-sandbox-indicator-${AGENT_PUBKEY}`,
    );
    await expect(indicator).toBeVisible({ timeout: 10_000 });
    await waitForAnimations(page);
    // Capture the whole agent card the indicator sits on (a running managed
    // agent renders as a `managed-agent-*` card).
    await page
      .locator(`[data-testid="managed-agent-${AGENT_PUBKEY}"]`)
      .screenshot({ path: `${SHOTS}/05-card-indicator.png` });
  });

  // Shot 04: a destroyed sandbox removes the card entirely.
  test("04-destroyed-removes-card", async ({ page }) => {
    await gotoAgentProfile(page);
    await emitSandbox(page, { viewerUrl: MOCK_VIEWER });

    const preview = page.getByTestId(`agent-sandbox-preview-${AGENT_PUBKEY}`);
    await expect(preview).toBeVisible({ timeout: 10_000 });

    await page.evaluate((ownerPubkey) => {
      const emit = (
        window as Window & {
          __BUZZ_E2E_EMIT_MOCK_SANDBOX__?: (input: unknown) => unknown;
        }
      ).__BUZZ_E2E_EMIT_MOCK_SANDBOX__;
      emit?.({ ownerPubkey, destroyed: true });
    }, AGENT_PUBKEY);

    await expect(preview).toHaveCount(0, { timeout: 5_000 });
  });
});
