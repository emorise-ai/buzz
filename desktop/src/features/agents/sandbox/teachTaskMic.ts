import { invokeTauri } from "@/shared/api/tauri";

/**
 * Mic capture + on-device transcription for "Teach a task."
 *
 * Reuses the same worklet the huddle feature uses to tap the mic
 * (`/worklet.js`'s `stt-tap-processor`, ~20ms Float32 batches at 48 kHz mono)
 * but does NOT feed those batches into the huddle's live `push_audio_pcm`
 * command — that command fans out into the *huddle's* STT pipeline, which
 * only exists while a huddle is active. Teaching a task has nothing to do
 * with huddles, so this collects the batches locally and, on stop, hands the
 * whole clip to a standalone one-shot transcription command
 * (`transcribe_teaching_audio`) that spins up its own throwaway STT pipeline.
 *
 * No huddle files are touched or imported — only the static worklet module,
 * which is generic (any consumer can tap it for 48kHz mono PCM batches).
 */

const SAMPLE_RATE = 48_000;

export type TeachTaskMicHandle = {
  /** Stop capturing and release the mic. Safe to call once. */
  stop: () => TeachTaskMicResult;
};

export type TeachTaskMicResult = {
  /** Raw f32 LE PCM samples, concatenated in capture order — what
   *  `transcribe_teaching_audio` expects. Empty if nothing was captured. */
  pcmBytes: Uint8Array;
  /** The same audio re-encoded as a 16-bit PCM WAV file — self-describing
   *  bytes a muxer (or any audio tool) can use directly, unlike the bare
   *  float buffer above. `null` if nothing was captured. */
  wavBytes: Uint8Array | null;
};

/**
 * Start tapping the microphone. Throws if `getUserMedia` is denied or no
 * mic is available — callers should catch and surface a toast, same as any
 * other mic-permission failure in this app.
 */
export async function startTeachingMic(): Promise<TeachTaskMicHandle> {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      sampleRate: SAMPLE_RATE,
    },
  });
  const audioTrack = stream.getAudioTracks()[0];

  const audioContext = new AudioContext({ sampleRate: SAMPLE_RATE });
  if (audioContext.state === "suspended") {
    await audioContext.resume();
  }
  // The worklet + the STT resampler both assume exactly 48 kHz; getUserMedia's
  // sampleRate constraint is advisory and macOS often ignores it. Warn if the
  // real rate differs, since that would silently garble the transcript.
  if (audioContext.sampleRate !== SAMPLE_RATE) {
    console.warn(
      `[teachTaskMic] AudioContext is ${audioContext.sampleRate} Hz, expected ${SAMPLE_RATE} Hz — transcription may be wrong.`,
    );
  }
  await audioContext.audioWorklet.addModule("/worklet.js");

  const source = audioContext.createMediaStreamSource(
    new MediaStream([audioTrack]),
  );
  const workletNode = new AudioWorkletNode(audioContext, "stt-tap-processor");
  source.connect(workletNode);

  const batches: Float32Array[] = [];
  let totalSamples = 0;
  workletNode.port.onmessage = (event: MessageEvent<Float32Array>) => {
    batches.push(event.data);
    totalSamples += event.data.length;
  };

  let stopped = false;
  return {
    stop: () => {
      if (stopped) {
        return { pcmBytes: new Uint8Array(0), wavBytes: null };
      }
      stopped = true;
      workletNode.port.onmessage = null;
      source.disconnect();
      workletNode.disconnect();
      void audioContext.close();
      for (const track of stream.getTracks()) {
        track.stop();
      }

      if (totalSamples === 0) {
        return { pcmBytes: new Uint8Array(0), wavBytes: null };
      }
      const merged = new Float32Array(totalSamples);
      let offset = 0;
      for (const batch of batches) {
        merged.set(batch, offset);
        offset += batch.length;
      }
      const pcmBytes = new Uint8Array(merged.buffer);
      const wavBytes = encodeWav(merged, SAMPLE_RATE);
      return { pcmBytes, wavBytes };
    },
  };
}

/** Send the whole captured clip to the on-device Parakeet model and return
 *  the transcript. Empty string (not an error) means no speech was detected.
 *  Rejects if the STT model hasn't finished its background download yet. */
export async function transcribeTeachingAudio(
  pcmBytes: Uint8Array,
): Promise<string> {
  if (pcmBytes.length === 0) return "";
  return invokeTauri<string>("transcribe_teaching_audio", {
    pcmBytes: Array.from(pcmBytes),
  });
}

/** Encode mono f32 PCM samples as a 16-bit PCM WAV file. */
function encodeWav(samples: Float32Array, sampleRate: number): Uint8Array {
  const bytesPerSample = 2;
  const blockAlign = bytesPerSample; // mono
  const dataSize = samples.length * bytesPerSample;
  const buffer = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buffer);

  writeAscii(view, 0, "RIFF");
  view.setUint32(4, 36 + dataSize, true);
  writeAscii(view, 8, "WAVE");
  writeAscii(view, 12, "fmt ");
  view.setUint32(16, 16, true); // fmt chunk size
  view.setUint16(20, 1, true); // PCM format
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * blockAlign, true); // byte rate
  view.setUint16(32, blockAlign, true);
  view.setUint16(34, 16, true); // bits per sample
  writeAscii(view, 36, "data");
  view.setUint32(40, dataSize, true);

  let offset = 44;
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    const int16 = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
    view.setInt16(offset, int16, true);
    offset += 2;
  }
  return new Uint8Array(buffer);
}

function writeAscii(view: DataView, offset: number, text: string): void {
  for (let i = 0; i < text.length; i += 1) {
    view.setUint8(offset + i, text.charCodeAt(i));
  }
}
