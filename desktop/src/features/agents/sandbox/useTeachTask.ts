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
import {
  startTeachingMic,
  transcribeTeachingAudio,
  type TeachTaskMicHandle,
} from "./teachTaskMic";

/**
 * "Teach a task" hands the human the screen, records it via the broker's
 * recording endpoints (owner-or-manager gated, same as every other
 * computer-use call), captures the human's SPOKEN narration over the mic
 * instead of typed text, then on Done stops both, transcribes the narration
 * on-device, uploads the finished (now audio+video) clip, and messages the
 * agent's channel with the URL + transcript so the agent can watch it and
 * propose a skill. `teaching` gates the banner; `finishing` covers the
 * stop→transcribe→upload→send window so Done/Cancel can't double-fire.
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
  const [listening, setListening] = React.useState(false);
  const micRef = React.useRef<TeachTaskMicHandle | null>(null);
  const identityQuery = useIdentityQuery();
  const sendMessageMutation = useSendMessageMutation(null, identityQuery.data);
  const openDmMutation = useOpenDmMutation();

  /** Reset all teaching state — call alongside a host surface's own
   *  session-reset (e.g. a freshly reopened dialog's `open` effect). */
  const resetTeaching = React.useCallback(() => {
    micRef.current?.stop();
    micRef.current = null;
    setTeaching(false);
    setFinishing(false);
    setListening(false);
  }, []);

  async function startTeaching() {
    setUserInControl(true);
    setTeaching(true);
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
    // Mic capture is best-effort: a denied/unavailable mic shouldn't block
    // the screen recording the human explicitly asked for — they just won't
    // get a transcript alongside it.
    try {
      micRef.current = await startTeachingMic();
      setListening(true);
    } catch (err) {
      console.error("[useTeachTask] mic capture failed to start:", err);
      toast.error(
        "Could not access the microphone — recording video only, no narration.",
      );
    }
    toast.info("Recording — narrate the task out loud, then click Done.");
  }

  async function cancelTeaching() {
    setFinishing(true);
    setListening(false);
    micRef.current?.stop();
    micRef.current = null;
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
    }
  }

  async function doneTeaching() {
    if (!ownerPubkey) {
      toast.error("This agent has no known pubkey to message.");
      return;
    }
    setFinishing(true);
    setListening(false);
    const mic = micRef.current;
    micRef.current = null;
    const { pcmBytes, wavBytes } = mic?.stop() ?? {
      pcmBytes: new Uint8Array(0),
      wavBytes: null,
    };
    try {
      // Transcribe first — the recording upload below hands the same WAV
      // bytes to the broker to bake into the video, so a transcription
      // failure (e.g. model not downloaded yet) shouldn't also block that.
      let transcript = "";
      try {
        transcript = await transcribeTeachingAudio(pcmBytes);
      } catch (err) {
        console.error("[useTeachTask] transcription failed:", err);
        // Non-fatal — send the recording without a transcript rather than
        // dropping the whole teaching handoff over an STT hiccup.
      }

      const recording = await stopSandboxRecording(
        sandboxId,
        wavBytes ? { bytes: wavBytes, ext: "wav" } : undefined,
      );
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
      const narrationText = transcript.trim();
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
  }

  return {
    teaching,
    finishing,
    listening,
    startTeaching,
    cancelTeaching,
    doneTeaching,
    resetTeaching,
  };
}
