import { expect, test, type Page } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const AGENT_PUBKEY = TEST_IDENTITIES.tyler.pubkey;
const AGENT_NAME = "Tyler Agent";
const ENV_KEY = "ANTHROPIC_API_KEY";
const ACTIVE_RELAY_URL = "ws://localhost:3000";
const OTHER_RELAY_URL = "ws://other-community.example";

async function invokeMockCommand(
  page: Page,
  command: string,
  payload?: Record<string, unknown>,
): Promise<unknown> {
  await page.waitForFunction(
    () =>
      typeof (window as Window & { __BUZZ_E2E_INVOKE_MOCK_COMMAND__?: unknown })
        .__BUZZ_E2E_INVOKE_MOCK_COMMAND__ === "function",
  );
  return page.evaluate(
    async ({ command: commandName, payload: commandPayload }) => {
      const invoke = (
        window as Window & {
          __BUZZ_E2E_INVOKE_MOCK_COMMAND__?: (
            name: string,
            payload?: Record<string, unknown>,
          ) => Promise<unknown>;
        }
      ).__BUZZ_E2E_INVOKE_MOCK_COMMAND__;
      if (!invoke) throw new Error("Mock invoke bridge is unavailable");
      return invoke(commandName, commandPayload);
    },
    { command, payload },
  );
}

function nudgeBody(agentPubkey = AGENT_PUBKEY): string {
  const payload = JSON.stringify({
    agent_name: AGENT_NAME,
    agent_pubkey: agentPubkey,
    requirements: [{ surface: "env_key", key: ENV_KEY }],
  });
  return `**${AGENT_NAME}** needs configuration before it can respond. Do not paste the credential into chat.\n\n\`\`\`buzz:config-nudge\n${payload}\n\`\`\``;
}

async function openCredentialDialog(page: Page): Promise<void> {
  await invokeMockCommand(page, "send_managed_agent_channel_message", {
    agentPubkey: AGENT_PUBKEY,
    channelId: GENERAL_CHANNEL_ID,
    content: nudgeBody(),
  });
  await page.getByTestId("channel-general").click();

  const card = page.locator("[data-config-nudge]").last();
  await expect(card).toContainText("Do not paste this into chat");
  await card.getByRole("button", { name: "Add securely" }).click();
  await expect(
    page.getByTestId("secure-agent-credential-dialog"),
  ).toBeVisible();
}

async function installCredentialFixture(
  page: Page,
  failures?: {
    save?: string[];
    restart?: string[];
    retry?: string[];
  },
): Promise<void> {
  await installMockBridge(
    page,
    {
      managedAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: AGENT_NAME,
          status: "running",
          channelNames: ["general"],
          envVars: { SIBLING_TOKEN: "keep-me" },
        },
      ],
      setManagedAgentCredentialErrors: failures?.save,
      setManagedAgentCredentialRestartErrors: failures?.restart,
      restartManagedAgentRuntimeErrors: failures?.retry,
      managedAgentRuntimes: [
        {
          pubkey: AGENT_PUBKEY,
          relayUrl: ACTIVE_RELAY_URL,
          lifecycle: "listening",
        },
        {
          pubkey: AGENT_PUBKEY,
          relayUrl: OTHER_RELAY_URL,
          lifecycle: "stopped",
        },
      ],
    },
    { relayWsUrl: ACTIVE_RELAY_URL },
  );
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await openCredentialDialog(page);
}

test("securely saves one credential, preserves siblings, and restarts", async ({
  page,
}) => {
  await installCredentialFixture(page);
  const secret = "test-secret-never-rendered";
  const input = page.getByTestId("secure-agent-credential-input");
  await expect(input).toHaveAttribute("type", "password");
  await input.fill(secret);
  await expect(page.locator("body")).not.toContainText(secret);

  await page.getByRole("button", { name: "Save and restart" }).click();
  await expect(page.getByTestId("secure-agent-credential-dialog")).toHaveCount(
    0,
  );

  const state = await page.evaluate(
    async ({ activeRelayUrl, agentPubkey, envKey, otherRelayUrl }) => {
      const invoke = (
        window as Window & {
          __BUZZ_E2E_INVOKE_MOCK_COMMAND__?: (
            name: string,
            payload?: Record<string, unknown>,
          ) => Promise<unknown>;
        }
      ).__BUZZ_E2E_INVOKE_MOCK_COMMAND__;
      if (!invoke) throw new Error("Mock invoke bridge is unavailable");
      const agents = (await invoke("list_managed_agents")) as Array<{
        pubkey: string;
        env_vars: Record<string, string>;
      }>;
      const runtimes = (await invoke("list_managed_agent_runtimes")) as Array<{
        pubkey: string;
        relayUrl: string;
        lifecycle: string;
      }>;
      const agent = agents.find(
        (candidate) => candidate.pubkey === agentPubkey,
      );
      return {
        credentialPresent: Boolean(agent?.env_vars[envKey]),
        siblingPreserved: agent?.env_vars.SIBLING_TOKEN === "keep-me",
        runtimeReady: runtimes.some(
          (runtime) =>
            runtime.pubkey === agentPubkey &&
            runtime.relayUrl === activeRelayUrl &&
            runtime.lifecycle === "ready",
        ),
        otherRuntimeUntouched: runtimes.some(
          (runtime) =>
            runtime.pubkey === agentPubkey &&
            runtime.relayUrl === otherRelayUrl &&
            runtime.lifecycle === "stopped",
        ),
      };
    },
    {
      activeRelayUrl: ACTIVE_RELAY_URL,
      agentPubkey: AGENT_PUBKEY,
      envKey: ENV_KEY,
      otherRelayUrl: OTHER_RELAY_URL,
    },
  );

  expect(state).toEqual({
    credentialPresent: true,
    siblingPreserved: true,
    runtimeReady: true,
    otherRuntimeUntouched: true,
  });
});

test("clears a saved credential before offering restart-only retry", async ({
  page,
}) => {
  await installCredentialFixture(page, {
    restart: ["runtime unavailable"],
    retry: ["runtime still unavailable"],
  });
  const secret = "test-partial-success-secret";
  await page.getByTestId("secure-agent-credential-input").fill(secret);
  await page.getByRole("button", { name: "Save and restart" }).click();

  const partial = page.getByTestId("credential-restart-failed");
  await expect(partial).toContainText("Credential saved");
  await expect(page.getByTestId("secure-agent-credential-input")).toHaveCount(
    0,
  );
  await expect(page.locator("body")).not.toContainText(secret);
  await expect(page.locator("body")).not.toContainText("runtime unavailable");

  const persisted = (await invokeMockCommand(
    page,
    "list_managed_agents",
  )) as Array<{ pubkey: string; env_vars: Record<string, string> }>;
  expect(
    persisted.find((agent) => agent.pubkey === AGENT_PUBKEY)?.env_vars,
  ).toMatchObject({
    [ENV_KEY]: secret,
    SIBLING_TOKEN: "keep-me",
  });

  await page.getByRole("button", { name: "Retry restart" }).click();
  await expect(page.getByRole("alert")).toContainText("restart retry failed");
  await expect(page.locator("body")).not.toContainText(
    "runtime still unavailable",
  );
  await page.getByRole("button", { name: "Retry restart" }).click();
  await expect(page.getByTestId("secure-agent-credential-dialog")).toHaveCount(
    0,
  );
});

test("keeps the masked value for a generic save retry without leaking errors", async ({
  page,
}) => {
  await installCredentialFixture(page, { save: ["disk path /private/store"] });
  const secret = "test-save-failure-secret";
  const input = page.getByTestId("secure-agent-credential-input");
  await input.fill(secret);
  await page.getByRole("button", { name: "Save and restart" }).click();

  await expect(page.getByRole("alert")).toContainText(
    "Buzz could not save this credential",
  );
  await expect(input).toHaveValue(secret);
  await expect(input).toHaveAttribute("type", "password");
  await expect(page.locator("body")).not.toContainText(secret);
  await expect(page.locator("body")).not.toContainText("/private/store");

  const agents = (await invokeMockCommand(
    page,
    "list_managed_agents",
  )) as Array<{
    pubkey: string;
    env_vars: Record<string, string>;
  }>;
  expect(
    agents.find((agent) => agent.pubkey === AGENT_PUBKEY)?.env_vars,
  ).toEqual({
    SIBLING_TOKEN: "keep-me",
  });
  const runtimes = (await invokeMockCommand(
    page,
    "list_managed_agent_runtimes",
  )) as Array<{ relayUrl: string; lifecycle: string }>;
  expect(runtimes).toContainEqual(
    expect.objectContaining({
      relayUrl: ACTIVE_RELAY_URL,
      lifecycle: "listening",
    }),
  );
});

test("does not expose secure entry for a mismatched message signer", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [
      {
        pubkey: AGENT_PUBKEY,
        name: AGENT_NAME,
        status: "running",
        channelNames: ["general"],
      },
    ],
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await invokeMockCommand(page, "send_managed_agent_channel_message", {
    agentPubkey: AGENT_PUBKEY,
    channelId: GENERAL_CHANNEL_ID,
    content: nudgeBody("cd".repeat(32)),
  });
  await page.getByTestId("channel-general").click();

  await expect(page.locator("[data-config-nudge]")).toHaveCount(0);
  await expect(page.getByText(/needs configuration/).last()).toBeVisible();
  await expect(page.getByRole("button", { name: /Add securely/ })).toHaveCount(
    0,
  );
});
