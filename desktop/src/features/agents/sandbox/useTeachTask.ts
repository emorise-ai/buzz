import * as React from "react";
import { toast } from "sonner";

import { useSendMessageMutation } from "@/features/messages/hooks";
import { buildImetaTags } from "@/features/messages/lib/imetaMediaMarkdown";
import { useOpenDmMutation } from "@/features/channels/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  startSandboxRecording,
  stopSandboxRecording,
  uploadRecording,
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

/** How long the pre-recording countdown runs, in whole seconds. The mic (for
 *  audio-video mode) starts warming up the moment the countdown begins, and
 *  the sandbox screen recording only starts once it reaches zero — so by the
 *  time ffmpeg's first frame lands, the mic has been live for the full
 *  countdown instead of the ~1s `getUserMedia`/`AudioContext.resume`/worklet
 *  warm-up eating into the start of the narration. */
const COUNTDOWN_SECONDS = 3;
const COUNTDOWN_TICK_MS = 1000;

/** A stopped-but-not-yet-sent recording, awaiting the human's confirmation in
 *  the preview dialog. `bytes` and `wavBytes` are kept around (not just the
 *  object URL) so `sendPreview` can upload and mux without re-stopping the
 *  sandbox recording. */
export type TeachTaskPreview = {
  /** Local `URL.createObjectURL` playback source — video + baked-in audio. */
  videoUrl: string;
  /** Raw mp4 bytes backing `videoUrl`, unsent. */
  bytes: Uint8Array;
  /** On-device transcript of the narration, or "" if none/video-only. */
  transcript: string;
  /** True while transcription is still running in the background — the preview
   *  opens as soon as the video is ready and the transcript backfills after,
   *  so the dialog can show "Transcribing…" instead of a misleading "no
   *  narration". False for video-only (no mic) and once the transcript lands. */
  transcribing: boolean;
  /** The same WAV bytes already muxed into the clip — kept only for
   *  reference/debugging, not re-sent (they're already baked into `bytes`). */
  wavBytes: Uint8Array | null;
};

/**
 * "Teach a task" hands the human the screen, records it via the broker's
 * recording endpoints (owner-or-manager gated, same as every other
 * computer-use call), captures the human's SPOKEN narration over the mic
 * instead of typed text, then on Done stops both, transcribes the narration
 * on-device, and opens a local PREVIEW (video playback + transcript) so the
 * human can watch/listen to what was actually captured before anything
 * leaves the machine. Only on explicit confirmation (`sendPreview`) does the
 * clip get uploaded and messaged into the agent's DM. `teaching` gates the
 * button's recording state; `preview` gates the preview dialog.
 *
 * Done returns the button to idle IMMEDIATELY and hands the multi-second work
 * (stop+flush ffmpeg, transcribe) off to the background with a progress
 * toast — so the button never freezes on a slow or hung server round-trip.
 * The preview then opens a beat later, once the bytes are ready.
 *
 * Shared by `SandboxViewerDialog` and the pop-out native window so the two
 * surfaces run one teaching flow instead of two copies drifting apart.
 *
 * Recording mode ("Audio + Video" vs "Video only") is a persisted preference
 * (`teachTaskModePreference`), not per-call state — the human picks it once
 * from the button's dropdown and it's remembered as the default next time.
 *
 * `startTeaching` runs a visible 3-2-1 countdown (`countdown`) before the
 * sandbox recording actually starts. For audio-video mode, the mic is warmed
 * up (`getUserMedia`/`AudioContext.resume`/worklet load, ~1s) DURING that
 * countdown instead of after the recording starts — previously the mic
 * warm-up raced the recording start and lost the first ~second of narration.
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
  // Seconds remaining in the pre-recording countdown, or `null` when not
  // counting down (idle, actively recording, or processing). Drives the
  // countdown overlay in both surfaces.
  const [countdown, setCountdown] = React.useState<number | null>(null);
  // True while `doneTeaching`'s background stop+transcribe work is in
  // flight — drives the "Processing recording…" overlay so the human isn't
  // staring at a stage that looks frozen for the several seconds ffmpeg
  // takes to flush.
  const [processing, setProcessing] = React.useState(false);
  const [preview, setPreview] = React.useState<TeachTaskPreview | null>(null);
  const mode = useTeachTaskMode();
  const micRef = React.useRef<TeachTaskMicHandle | null>(null);
  // True whenever a sandbox recording is running (ffmpeg live inside the
  // sandbox). A ref, not state, so the unmount cleanup below reads the latest
  // value without re-subscribing. Without this, closing the computer window
  // mid-recording unmounts the hook and orphans ffmpeg — it keeps recording in
  // the sandbox forever with no UI left to stop it.
  const recordingActiveRef = React.useRef(false);
  // Handle for the countdown's `setInterval`, so cancel/unmount/completion can
  // always clear it — a ref (not state) since it's imperative bookkeeping, not
  // something a render depends on.
  const countdownTimerRef = React.useRef<number | null>(null);
  // Guards the countdown→recording-start handoff so it fires EXACTLY once. The
  // start is a side effect and must never run inside a `setState` updater (React
  // may invoke updaters twice), which double-called the broker: the first call
  // started ffmpeg, the second got a 409 whose catch reset the UI to idle while
  // ffmpeg kept running — the "shows stopped but says already recording" bug.
  const startFiredRef = React.useRef(false);
  // Mirrors the countdown value for the interval callback to read without a
  // stale closure — the tick decrements this and starts recording at zero.
  const countdownRef = React.useRef<number | null>(null);
  // Mirrors `preview.videoUrl` so the unmount cleanup can revoke it without
  // depending on `preview` (which would re-run the effect on every preview
  // open/close and re-register the unmount handler needlessly).
  const previewUrlRef = React.useRef<string | null>(null);
  const identityQuery = useIdentityQuery();
  const sendMessageMutation = useSendMessageMutation(null, identityQuery.data);
  const openDmMutation = useOpenDmMutation();

  const clearCountdownTimer = React.useCallback(() => {
    if (countdownTimerRef.current != null) {
      window.clearInterval(countdownTimerRef.current);
      countdownTimerRef.current = null;
    }
    countdownRef.current = null;
  }, []);

  // On unmount (window/dialog closed) while a recording is still running, stop
  // it in the sandbox so no orphaned ffmpeg keeps capturing. Best-effort and
  // fire-and-forget — the surface is already going away, so there's nothing to
  // send and no error to surface; we just must not leak the recording. Also
  // revoke any open preview's object URL — otherwise the blob leaks for the
  // life of the webview.
  React.useEffect(() => {
    return () => {
      clearCountdownTimer();
      micRef.current?.stop();
      micRef.current = null;
      if (recordingActiveRef.current) {
        recordingActiveRef.current = false;
        void stopSandboxRecording(sandboxId).catch((err) => {
          console.warn("[useTeachTask] stop on unmount failed:", err);
        });
      }
      if (previewUrlRef.current) {
        URL.revokeObjectURL(previewUrlRef.current);
        previewUrlRef.current = null;
      }
    };
  }, [sandboxId, clearCountdownTimer]);

  /** Reset all teaching state — call alongside a host surface's own
   *  session-reset (e.g. a freshly reopened dialog's `open` effect). */
  const resetTeaching = React.useCallback(() => {
    clearCountdownTimer();
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
    if (previewUrlRef.current) {
      URL.revokeObjectURL(previewUrlRef.current);
      previewUrlRef.current = null;
    }
    setTeaching(false);
    setListening(false);
    setCountdown(null);
    setProcessing(false);
    setPreview(null);
  }, [sandboxId, clearCountdownTimer]);

  async function startTeaching(startMode: TeachTaskMode) {
    setUserInControl(true);
    setTeaching(true);
    startFiredRef.current = false;
    countdownRef.current = COUNTDOWN_SECONDS;
    setCountdown(COUNTDOWN_SECONDS);

    if (startMode === "audio-video") {
      // Warm the mic up DURING the countdown, not after the sandbox recording
      // has already started — `getUserMedia` + `AudioContext.resume` + the
      // worklet module load take close to a second to go live, and starting
      // ffmpeg first meant that second of narration was silently dropped from
      // every recording. Best-effort: a denied/unavailable mic shouldn't
      // block the screen recording the human explicitly asked for — they just
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
    }

    // Actually start the sandbox recording once the countdown elapses. Kept
    // OUT of any `setState` updater (updaters must stay pure) and guarded by
    // `startFiredRef` so it runs exactly once even if a tick double-fires.
    const beginRecording = async () => {
      if (startFiredRef.current) return;
      startFiredRef.current = true;
      clearCountdownTimer();
      try {
        await startSandboxRecording(sandboxId);
        recordingActiveRef.current = true;
        setCountdown(null);
        toast.info(
          startMode === "audio-video"
            ? "Recording — narrate the task out loud, click again when done."
            : "Recording video — click again when done.",
        );
      } catch (err) {
        console.error("[useTeachTask] recording start failed:", err);
        toast.error(
          err instanceof Error
            ? err.message
            : "Could not start recording. Try again.",
        );
        micRef.current?.stop();
        micRef.current = null;
        setListening(false);
        setCountdown(null);
        setTeaching(false);
      }
    };

    // Tick the visible number down each second; when it reaches zero, start the
    // recording. `countdownRef` is the source of truth (no stale closure); the
    // state mirror just drives the overlay. A cancel/reset/unmount clears the
    // timer first, so a queued tick can't resurrect a cancelled countdown.
    countdownTimerRef.current = window.setInterval(() => {
      const next = (countdownRef.current ?? 1) - 1;
      countdownRef.current = next;
      if (next <= 0) {
        void beginRecording();
      } else {
        setCountdown(next);
      }
    }, COUNTDOWN_TICK_MS);
  }

  function cancelTeaching() {
    // Discard is instant: exit the recording UI at once, then stop the sandbox
    // ffmpeg in the background (if it had actually started — a cancel during
    // the countdown means it never did, since `startSandboxRecording` only
    // runs once the countdown hits zero). The human doesn't care about the
    // result — they asked to throw it away — so there's no reason to hold the
    // button while the broker waits for ffmpeg to flush.
    clearCountdownTimer();
    micRef.current?.stop();
    micRef.current = null;
    setListening(false);
    setTeaching(false);
    setCountdown(null);
    if (recordingActiveRef.current) {
      recordingActiveRef.current = false;
      void stopSandboxRecording(sandboxId).catch((err) => {
        // Nothing to surface — the recording is being discarded regardless.
        console.warn("[useTeachTask] recording stop on cancel:", err);
      });
    }
  }

  function doneTeaching() {
    if (!ownerPubkey) {
      toast.error("This agent has no known pubkey to message.");
      return;
    }
    // Done clicked mid-countdown, before the sandbox recording ever started —
    // nothing was captured, so there's nothing to finish. Fall back to the
    // same instant-exit path `cancelTeaching` uses.
    if (!recordingActiveRef.current) {
      cancelTeaching();
      return;
    }
    // Grab the mic buffers, then EXIT the recording UI immediately. Stopping
    // the sandbox recording and transcribing are both multi-second server
    // round-trips; holding the button in "Finishing…" until they complete
    // made the UI feel frozen and, if either step hung, left it stuck
    // forever. Instead the button returns to idle at once and the new
    // `processing` overlay tracks the background work, then the preview
    // dialog opens once the bytes are ready — nothing is uploaded or sent
    // until the human confirms it there.
    // Show the processing overlay FIRST — before the synchronous mic teardown
    // below (audio-context close + WAV encode of the whole clip), which can
    // block the main thread long enough that the overlay would otherwise paint
    // seconds late.
    recordingActiveRef.current = false;
    setListening(false);
    setTeaching(false);
    setProcessing(true);

    const mic = micRef.current;
    micRef.current = null;
    const { pcmBytes, wavBytes } = mic?.stop() ?? {
      pcmBytes: new Uint8Array(0),
      wavBytes: null,
    };

    // Kick off transcription in PARALLEL and DON'T block the preview on it —
    // on-device STT runs the clip in real time (a 30s clip ≈ 30s), so waiting
    // for it before showing the video made "processing" drag on and, if STT
    // hung, the preview never opened at all. The preview appears as soon as the
    // video bytes are ready; the transcript fills in when it lands.
    const transcriptPromise: Promise<string> = pcmBytes.length
      ? transcribeTeachingAudio(pcmBytes).catch((err) => {
          console.error("[useTeachTask] transcription failed:", err);
          toast.warning(
            `Transcription failed: ${err instanceof Error ? err.message : "unknown"}`,
          );
          return "";
        })
      : Promise.resolve("");

    const process = async () => {
      // The preview only needs the video; fetch it and open the dialog right
      // away (the mp4 already has the audio muxed in, so playback has sound
      // regardless of the transcript).
      const bytes = await stopSandboxRecording(
        sandboxId,
        wavBytes ? { bytes: wavBytes, ext: "wav" } : undefined,
      );
      const videoUrl = URL.createObjectURL(
        new Blob([bytes.buffer as ArrayBuffer], { type: "video/mp4" }),
      );
      previewUrlRef.current = videoUrl;
      const transcribing = pcmBytes.length > 0;
      setProcessing(false);
      setPreview({ videoUrl, bytes, transcript: "", transcribing, wavBytes });

      // Backfill the transcript into the open preview once STT finishes, and
      // clear the "transcribing" flag either way.
      const transcript = await transcriptPromise;
      setPreview((current) =>
        current && current.videoUrl === videoUrl
          ? { ...current, transcript, transcribing: false }
          : current,
      );
    };

    void process().catch((err) => {
      console.error("[useTeachTask] stopping recording failed:", err);
      toast.error(
        err instanceof Error
          ? err.message
          : "Could not finish the recording. Try again.",
      );
      setProcessing(false);
    });
  }

  /** Human confirmed the preview — upload the clip, open the DM, and send
   *  the teaching handoff. Clears the preview (and revokes its object URL)
   *  immediately since the bytes are already captured in the closure. */
  function sendPreview() {
    if (!preview || !ownerPubkey) return;
    const { bytes, transcript, videoUrl } = preview;

    const send = async () => {
      const recording = await uploadRecording(bytes);
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

    toast.promise(send(), {
      loading: "Sending to the agent…",
      success: "Sent the recording to the agent.",
      error: (err) => {
        console.error("[useTeachTask] sending teaching failed:", err);
        return err instanceof Error
          ? err.message
          : "Could not send the recording. Try again.";
      },
    });
    URL.revokeObjectURL(videoUrl);
    previewUrlRef.current = null;
    setPreview(null);
  }

  /** Human declined the preview — nothing is uploaded or sent. The sandbox
   *  recording is already stopped (before the preview ever opened), so
   *  there's nothing left running to clean up; just release the local blob. */
  function discardPreview() {
    if (!preview) return;
    URL.revokeObjectURL(preview.videoUrl);
    previewUrlRef.current = null;
    setPreview(null);
  }

  return {
    teaching,
    listening,
    countdown,
    processing,
    preview,
    mode,
    setMode: setTeachTaskMode,
    startTeaching,
    cancelTeaching,
    doneTeaching,
    sendPreview,
    discardPreview,
    resetTeaching,
  };
}
