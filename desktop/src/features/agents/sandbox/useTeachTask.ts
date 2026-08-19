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
import {
  setTeachTaskMode,
  useTeachTaskMode,
  type TeachTaskMode,
} from "./teachTaskModePreference";

/**
 * "Teach a task" hands the human the screen, records it via the broker's
 * recording endpoints (owner-or-manager gated, same as every other
 * computer-use call), captures the human's SPOKEN narration over the mic
 * instead of typed text, then on Done stops both, transcribes the narration
 * on-device, uploads the finished (now audio+video) clip, and messages the
 * agent's channel with the URL + transcript so the agent can watch it and
 * propose a skill. `teaching` gates the button's recording state.
 *
 * Done and Discard both return the button to idle IMMEDIATELY and hand the
 * multi-second work (stop+flush ffmpeg, transcribe, upload, send) off to the
 * background with a progress toast — so the button never freezes on a slow or
 * hung server round-trip.
 *
 * Shared by `SandboxViewerDialog` and the pop-out native window so the two
 * surfaces run one teaching flow instead of two copies drifting apart.
 *
 * Recording mode ("Audio + Video" vs "Video only") is a persisted preference
 * (`teachTaskModePreference`), not per-call state — the human picks it once
 * from the button's dropdown and it's remembered as the default next time.
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
  const [listening, setListening] = React.useState(false);
  const mode = useTeachTaskMode();
  const micRef = React.useRef<TeachTaskMicHandle | null>(null);
  // True whenever a sandbox recording is running (ffmpeg live inside the
  // sandbox). A ref, not state, so the unmount cleanup below reads the latest
  // value without re-subscribing. Without this, closing the computer window
  // mid-recording unmounts the hook and orphans ffmpeg — it keeps recording in
  // the sandbox forever with no UI left to stop it.
  const recordingActiveRef = React.useRef(false);
  const identityQuery = useIdentityQuery();
  const sendMessageMutation = useSendMessageMutation(null, identityQuery.data);
  const openDmMutation = useOpenDmMutation();

  // On unmount (window/dialog closed) while a recording is still running, stop
  // it in the sandbox so no orphaned ffmpeg keeps capturing. Best-effort and
  // fire-and-forget — the surface is already going away, so there's nothing to
  // send and no error to surface; we just must not leak the recording.
  React.useEffect(() => {
    return () => {
      micRef.current?.stop();
      micRef.current = null;
      if (recordingActiveRef.current) {
        recordingActiveRef.current = false;
        void stopSandboxRecording(sandboxId).catch((err) => {
          console.warn("[useTeachTask] stop on unmount failed:", err);
        });
      }
    };
  }, [sandboxId]);

  /** Reset all teaching state — call alongside a host surface's own
   *  session-reset (e.g. a freshly reopened dialog's `open` effect). */
  const resetTeaching = React.useCallback(() => {
    micRef.current?.stop();
    micRef.current = null;
    // If a reset lands mid-recording (e.g. the dialog reopened onto a new
    // session while one was live), stop the sandbox ffmpeg too — same leak the
    // unmount guard closes.
    if (recordingActiveRef.current) {
      recordingActiveRef.current = false;
      void stopSandboxRecording(sandboxId).catch((err) => {
        console.warn("[useTeachTask] stop on reset failed:", err);
      });
    }
    setTeaching(false);
    setListening(false);
  }, [sandboxId]);

  async function startTeaching(startMode: TeachTaskMode) {
    setUserInControl(true);
    setTeaching(true);
    try {
      await startSandboxRecording(sandboxId);
      recordingActiveRef.current = true;
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
    if (startMode === "audio-video") {
      // Mic capture is best-effort: a denied/unavailable mic shouldn't block
      // the screen recording the human explicitly asked for — they just
      // won't get a transcript alongside it.
      try {
        micRef.current = await startTeachingMic();
        setListening(true);
      } catch (err) {
        console.error("[useTeachTask] mic capture failed to start:", err);
        toast.error(
          "Could not access the microphone — recording video only, no narration.",
        );
      }
      toast.info(
        "Recording — narrate the task out loud, click again when done.",
      );
    } else {
      toast.info("Recording video — click again when done.");
    }
  }

  function cancelTeaching() {
    // Discard is instant: exit the recording UI at once, then stop the sandbox
    // ffmpeg in the background. The human doesn't care about the result — they
    // asked to throw it away — so there's no reason to hold the button while
    // the broker waits for ffmpeg to flush.
    micRef.current?.stop();
    micRef.current = null;
    recordingActiveRef.current = false;
    setListening(false);
    setTeaching(false);
    void stopSandboxRecording(sandboxId).catch((err) => {
      // Nothing to surface — the recording is being discarded regardless.
      console.warn("[useTeachTask] recording stop on cancel:", err);
    });
  }

  function doneTeaching() {
    if (!ownerPubkey) {
      toast.error("This agent has no known pubkey to message.");
      return;
    }
    // Grab the mic buffers, then EXIT the recording UI immediately. Everything
    // below (stop+flush the sandbox ffmpeg, transcribe, upload, send) is a
    // multi-second server round-trip; holding the button in "Finishing…" until
    // it all completes made the UI feel frozen and, if any step hung, left it
    // stuck forever. Instead the button returns to idle at once and a toast
    // tracks the background work.
    const mic = micRef.current;
    micRef.current = null;
    const { pcmBytes, wavBytes } = mic?.stop() ?? {
      pcmBytes: new Uint8Array(0),
      wavBytes: null,
    };
    recordingActiveRef.current = false;
    setListening(false);
    setTeaching(false);

    const process = async () => {
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
    };

    // Background work with a progress toast — the button is already free.
    toast.promise(process(), {
      loading: "Processing recording — sending to the agent…",
      success: "Sent the recording to the agent.",
      error: (err) => {
        console.error("[useTeachTask] finishing teaching failed:", err);
        return err instanceof Error
          ? err.message
          : "Could not finish teaching. Try again.";
      },
    });
  }

  return {
    teaching,
    listening,
    mode,
    setMode: setTeachTaskMode,
    startTeaching,
    cancelTeaching,
    doneTeaching,
    resetTeaching,
  };
}
