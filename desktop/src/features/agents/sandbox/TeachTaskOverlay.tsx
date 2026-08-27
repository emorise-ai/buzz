import { Loader2 } from "lucide-react";

/**
 * Visual state overlay for the "Teach a task" recording stage, rendered as a
 * sibling of `SandboxStage` inside its `relative` wrapper (see
 * `SandboxViewerDialog` and `ComputerWindowScreen`). Three mutually exclusive
 * states driven straight off `useTeachTask`'s return:
 *
 * - `countdown` (3-2-1 before the sandbox recording actually starts): a large
 *   centered number over a dimmed backdrop. The mic is already warming up
 *   behind this, so by the time it hits zero, audio and video start live
 *   together — see `useTeachTask.startTeaching`.
 * - `recording` (countdown finished, ffmpeg live in the sandbox): a red ring
 *   framing the stage so it's unmistakable the screen is being captured.
 * - `processing` (Done clicked, stop+transcribe running in the background):
 *   a centered spinner + label + indeterminate bar, so the several-second
 *   gap before the preview dialog opens doesn't read as a freeze.
 *
 * Purely visual — `pointer-events-none` throughout, so it never blocks clicks
 * on the underlying live stream or (during processing) the rest of the app.
 */
export function TeachTaskOverlay({
  countdown,
  recording,
  processing,
}: {
  countdown: number | null;
  recording: boolean;
  processing: boolean;
}) {
  if (countdown != null) {
    return (
      <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center bg-background/60">
        <span
          key={countdown}
          className="animate-in fade-in zoom-in text-7xl font-semibold text-foreground duration-300"
        >
          {countdown}
        </span>
      </div>
    );
  }

  if (processing) {
    return (
      <div className="pointer-events-none absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 bg-background/60">
        <Loader2 className="h-8 w-8 animate-spin text-foreground" />
        <p className="text-sm font-medium text-foreground">
          Processing recording…
        </p>
        <div className="h-1 w-48 overflow-hidden rounded-full bg-muted">
          <div className="h-full w-full animate-pulse rounded-full bg-foreground/60" />
        </div>
      </div>
    );
  }

  if (recording) {
    return (
      <div className="pointer-events-none absolute inset-0 z-10 rounded-[inherit] ring-2 ring-inset ring-red-500" />
    );
  }

  return null;
}
