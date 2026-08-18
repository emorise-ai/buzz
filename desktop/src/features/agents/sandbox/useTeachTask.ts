import * as React from "react";
import { toast } from "sonner";

import { useSendMessageMutation } from "@/features/messages/hooks";
import { buildImetaTags } from "@/features/messages/lib/imetaMediaMarkdown";
import { useOpenDmMutation } from "@/features/channels/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  startSandboxRecording,
  stopSandboxRecording,
} from "./sandboxRecording";

/**
 * "Teach a task" hands the human the screen, records it via the broker's
 * recording endpoints (owner-or-manager gated, same as every other
 * computer-use call), then on Done uploads the finished clip and messages
 * the agent's channel with the URL + narration so the agent can watch it and
 * propose a skill. `teaching` gates the banner; `finishing` covers the
 * stop→upload→send window so Done/Cancel can't double-fire.
 *
 * Extracted verbatim from `SandboxViewerDialog` (no behavior change) so the
 * fullscreen dialog and the pop-out native window share one teaching flow
 * instead of two copies drifting apart.
 */
export function useTeachTask({
  sandboxId,
  ownerPubkey,
  setUserInControl,
}: {
  sandboxId: string;
  ownerPubkey?: string | null;
  setUserInControl: (userInControl: boolean) => void;
}) {
  const [teaching, setTeaching] = React.useState(false);
  const [finishing, setFinishing] = React.useState(false);
  const [narration, setNarration] = React.useState("");
  const identityQuery = useIdentityQuery();
  const sendMessageMutation = useSendMessageMutation(null, identityQuery.data);
  const openDmMutation = useOpenDmMutation();

  /** Reset all teaching state — call alongside a host surface's own
   *  session-reset (e.g. a freshly reopened dialog's `open` effect). */
  const resetTeaching = React.useCallback(() => {
    setTeaching(false);
    setFinishing(false);
    setNarration("");
  }, []);

  async function startTeaching() {
    setUserInControl(true);
    setTeaching(true);
    setNarration("");
    try {
      await startSandboxRecording(sandboxId);
    } catch (err) {
      console.error("[useTeachTask] recording start failed:", err);
      toast.error(
        err instanceof Error
          ? err.message
          : "Could not start recording. Try again.",
      );
      setTeaching(false);
      return;
    }
    toast.info("Recording — show the agent the task, then click Done.");
  }

  async function cancelTeaching() {
    setFinishing(true);
    try {
      await stopSandboxRecording(sandboxId);
    } catch (err) {
      // Discarding regardless — the human asked to cancel, and a stop
      // failure here (e.g. nothing was recording) shouldn't trap them in
      // teaching mode.
      console.warn("[useTeachTask] recording stop on cancel:", err);
    } finally {
      setFinishing(false);
      setTeaching(false);
      setNarration("");
    }
  }

  async function doneTeaching() {
    if (!ownerPubkey) {
      toast.error("This agent has no known pubkey to message.");
      return;
    }
    setFinishing(true);
    try {
      const recording = await stopSandboxRecording(sandboxId);
      // Send the teaching handoff into the DM with this agent. A DM with the
      // agent always exists (or is created on demand) by construction, so we
      // never depend on the human and agent sharing a group channel — the
      // earlier membership-based lookup returned null for DM-only agents and
      // dropped the whole handoff.
      const dmChannel = await openDmMutation.mutateAsync({
        pubkeys: [ownerPubkey],
      });
      const channelId = dmChannel.id;
      // Attach the clip as an imeta media tag (not a URL in the text) so it
      // renders as a playable video in the timeline. The agent still gets the
      // fetchable URL from the imeta tag.
      const mediaTags = buildImetaTags([recording]);
      const narrationText = narration.trim();
      const narrationLine = narrationText
        ? ` My narration: "${narrationText}".`
        : "";
      const content = `I just taught you a task by demonstration — the screen recording is attached.${narrationLine} Watch the recording, then propose a named skill (name + summary + ordered steps + any inputs) and ask me to confirm before saving it.`;
      // Mention the agent so its harness's trigger filter wakes it — a bare
      // message with no `#p` at the agent can be ignored by mention-gated
      // agents, which would silently drop the whole teaching handoff.
      await sendMessageMutation.mutateAsync({
        channelId,
        content,
        mentionPubkeys: [ownerPubkey],
        mediaTags,
      });
      toast.success("Sent the recording to the agent.");
    } catch (err) {
      console.error("[useTeachTask] finishing teaching failed:", err);
      toast.error(
        err instanceof Error
          ? err.message
          : "Could not finish teaching. Try again.",
      );
      return;
    } finally {
      setFinishing(false);
    }
    setTeaching(false);
    setNarration("");
  }

  return {
    teaching,
    finishing,
    narration,
    setNarration,
    startTeaching,
    cancelTeaching,
    doneTeaching,
    resetTeaching,
  };
}
