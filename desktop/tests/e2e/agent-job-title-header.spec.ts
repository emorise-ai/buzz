import { expect, test } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

test("shows a configured agent job title in the one-to-one DM header", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [
      {
        pubkey: TEST_IDENTITIES.alice.pubkey,
        name: "Emely",
        jobTitle: "Principal Researcher",
        channelNames: ["alice-tyler"],
      },
    ],
  });
  await page.goto("/");

  await expect(page.getByTestId("channel-job-title-alice-tyler")).toHaveText(
    "Principal Researcher",
  );
  await page.getByTestId("channel-alice-tyler").click();

  await expect(page.getByTestId("chat-title")).toHaveText("alice-tyler");
  await expect(page.getByTestId("chat-subtitle")).toHaveText(
    "· Principal Researcher",
  );
});
