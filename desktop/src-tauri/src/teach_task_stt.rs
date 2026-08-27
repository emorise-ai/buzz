//! One-shot, batch transcription for the "Teach a task" flow.
//!
//! The huddle feature already runs an on-device Parakeet STT pipeline
//! (`huddle::stt::SttPipeline`), but it's wired to the live huddle audio path
//! (started/stopped alongside huddle state, fed frame-by-frame from the
//! AudioWorklet in real time). Teaching a task needs a standalone
//! "transcribe this whole clip" operation with no huddle involved, so this
//! module spins up its own short-lived `SttPipeline`, feeds it the entire
//! recorded clip, and returns the concatenated transcript.
//!
//! Reuses `SttPipeline` and the model-readiness helpers as-is — no huddle
//! files are touched. The feed pacing (real-time batches + trailing silence)
//! mirrors `huddle::latency_bench`, which already proves this pattern works
//! against the same pipeline.

use std::time::Duration;

use crate::huddle::{models, stt::SttPipeline, HumanFloor};

/// Batch size matching the AudioWorklet's push cadence (100 ms at 48 kHz mono
/// f32). `SttPipeline`'s internal queue is sized around this same cadence, so
/// feeding any faster risks silently dropping frames on the bounded channel.
const BATCH_SAMPLES: usize = 4_800;
const BATCH_BYTES: usize = BATCH_SAMPLES * 4;

/// Inter-batch feed delay — small enough to run far faster than real time,
/// large enough that the bounded audio queue drains between pushes rather than
/// overflowing (which would silently drop frames on `try_send`).
const FEED_DELAY: Duration = Duration::from_millis(8);

/// Trailing silence fed after the real clip so the VAD's silence-flush window
/// closes and the final utterance is decoded instead of left buffered.
const TRAILING_SILENCE_BATCHES: usize = 10;

/// Delay between trailing-silence batches — enough to let the VAD advance its
/// silence window without pacing to real time.
const TRAILING_SILENCE_DELAY: Duration = Duration::from_millis(20);

/// How long to keep polling for transcript segments after the feed thread
/// finishes, in case a decode is still in flight.
const DRAIN_GRACE: Duration = Duration::from_millis(800);

/// Transcribe a full clip of raw PCM audio (f32 LE, 48 kHz mono — the same
/// format the AudioWorklet sends to `push_audio_pcm`) using the on-device
/// Parakeet model, entirely offline.
///
/// Returns `Err` if the STT model has not finished downloading yet. Returns
/// `Ok("")` (not an error) if the model is ready but no speech was detected —
/// callers should treat an empty transcript as "the user said nothing."
#[tauri::command]
pub async fn transcribe_teaching_audio(pcm_bytes: Vec<u8>) -> Result<String, String> {
    if !models::is_stt_ready() {
        return Err("speech-to-text model is not downloaded yet".to_string());
    }
    let model_dir = models::stt_model_dir().ok_or("STT model directory not found")?;

    // Feeding the pipeline and blocking on the sherpa-onnx decode are both
    // synchronous/CPU-bound; run the whole thing on a blocking thread so it
    // doesn't stall the async runtime.
    tokio::task::spawn_blocking(move || transcribe_blocking(model_dir, pcm_bytes))
        .await
        .map_err(|e| format!("transcription task panicked: {e}"))?
}

fn transcribe_blocking(
    model_dir: std::path::PathBuf,
    pcm_bytes: Vec<u8>,
) -> Result<String, String> {
    let (stt, mut text_rx) = SttPipeline::new(model_dir, None, None, HumanFloor::new(), None)?;

    // Feed the clip in fixed-size batches. The bounded audio queue (`try_send`,
    // drops on full) must not overflow, but pacing to *real time* meant a 30 s
    // clip took ~30 s to transcribe — far too slow for a "finish → preview"
    // flow. Instead feed much faster than real time with a small inter-batch
    // delay that still lets the decode worker drain the queue between pushes.
    // FEED_DELAY << real-time-per-batch (100 ms), so this runs ~an order of
    // magnitude faster while staying comfortably under the queue's capacity.
    let mut cursor = 0usize;
    while cursor < pcm_bytes.len() {
        let end = (cursor + BATCH_BYTES).min(pcm_bytes.len());
        stt.push_audio(pcm_bytes[cursor..end].to_vec())?;
        cursor = end;
        std::thread::sleep(FEED_DELAY);
    }

    // Trailing silence forces the VAD's silence-flush window to close so the
    // last utterance decodes instead of sitting in the buffer waiting for
    // audio that will never arrive.
    let silence = vec![0u8; BATCH_BYTES];
    for _ in 0..TRAILING_SILENCE_BATCHES {
        stt.push_audio(silence.clone())?;
        std::thread::sleep(TRAILING_SILENCE_DELAY);
    }

    // Collect every emitted segment (VAD may split one narration into several
    // utterances on pauses) and join them into one transcript. By the time
    // the feed loop (including trailing silence) returns, every utterance in
    // the clip has already crossed the silence-flush threshold, so any
    // remaining transcripts are either already queued or a decode still in
    // flight — a short grace window covers the latter. `is_finished()` only
    // detects a crashed worker, not "done processing," so it can't gate this
    // loop; a bounded poll-until-quiet is what actually reflects "no more
    // segments are coming."
    let mut segments: Vec<String> = Vec::new();
    let mut quiet_for = Duration::ZERO;
    while quiet_for < DRAIN_GRACE {
        match text_rx.try_recv() {
            Ok(text) => {
                segments.push(text);
                quiet_for = Duration::ZERO;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                if stt.is_finished() {
                    break;
                }
                let step = Duration::from_millis(50);
                std::thread::sleep(step);
                quiet_for += step;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    stt.shutdown();
    drop(stt);

    Ok(segments.join(" ").trim().to_string())
}
