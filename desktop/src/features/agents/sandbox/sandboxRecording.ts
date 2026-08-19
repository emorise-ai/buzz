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
 * Stop an in-progress recording and return the raw finished mp4 bytes. Does
 * NOT upload anywhere — the caller decides what to do with the clip first
 * (e.g. show it in a preview dialog) before committing to a Blossom upload.
 *
 * `sandbox_recording_stop` fetches the mp4 bytes from the broker, which does
 * not host them anywhere durable — the file lives only inside the sandbox's
 * own filesystem, so this is the only chance to grab them.
 *
 * `audio` is the "Teach a task" mic capture (WAV bytes) plus its extension.
 * When provided, the broker bakes it into the mp4 as the video's audio track
 * before handing the bytes back. Omitting it is the original silent-video
 * behavior.
 */
export async function stopSandboxRecording(
  sandboxId: string,
  audio?: { bytes: Uint8Array; ext: string },
): Promise<Uint8Array> {
  const bytes = await invokeTauri<number[]>("sandbox_recording_stop", {
    sandboxId,
    audio: audio ? Array.from(audio.bytes) : undefined,
    audioExt: audio?.ext,
  });
  return new Uint8Array(bytes);
}

/**
 * Upload a previously-stopped recording's raw mp4 bytes to media storage
 * (Blossom, via the same `uploadMediaBytes` path used for pasted/dragged
 * media) and return the full descriptor. Split from `stopSandboxRecording` so
 * a "Teach a task" preview can show the local clip before committing to an
 * upload the human might discard instead.
 */
export async function uploadRecording(
  bytes: Uint8Array,
): Promise<BlobDescriptor> {
  return uploadMediaBytes(Array.from(bytes), "teach-task-recording.mp4");
}
