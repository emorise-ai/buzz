import {
  invokeTauri,
  uploadMediaBytes,
  type BlobDescriptor,
} from "@/shared/api/tauri";

/**
 * Start capturing a sandbox's screen — the "Teach a task" flow. The Tauri
 * backend signs the broker call with the user's key, same as every other
 * computer-use command (`sandbox_heartbeat`, `mint_sandbox_viewer_url`, …).
 *
 * Throws (with the broker's own message, e.g. "a recording is already in
 * progress") on a 409 — callers surface that via a toast.
 */
export async function startSandboxRecording(sandboxId: string): Promise<void> {
  await invokeTauri("sandbox_recording_start", { sandboxId });
}

/**
 * Stop an in-progress recording, upload the finished mp4 to media storage,
 * and return the full media descriptor.
 *
 * Two Tauri round-trips: `sandbox_recording_stop` fetches the raw mp4 bytes
 * from the broker (which does not host them anywhere durable — the file lives
 * only inside the sandbox's own filesystem), then `uploadMediaBytes` — the
 * same Blossom upload path used for pasted/dragged media — turns those bytes
 * into a fetchable blob. The full descriptor (url + sha256 + mime + size) lets
 * the caller attach the clip as a real imeta media tag so it renders as a
 * playable video in the message, not a bare URL.
 *
 * `audio` is the "Teach a task" mic capture (WAV bytes) plus its extension.
 * When provided, the broker bakes it into the mp4 as the video's audio track
 * before handing the bytes back — same one round-trip, now with sound.
 * Omitting it is the original silent-video behavior.
 */
export async function stopSandboxRecording(
  sandboxId: string,
  audio?: { bytes: Uint8Array; ext: string },
): Promise<BlobDescriptor> {
  const bytes = await invokeTauri<number[]>("sandbox_recording_stop", {
    sandboxId,
    audio: audio ? Array.from(audio.bytes) : undefined,
    audioExt: audio?.ext,
  });
  return uploadMediaBytes(bytes, "teach-task-recording.mp4");
}
