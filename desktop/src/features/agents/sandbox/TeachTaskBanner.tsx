import { Circle, Mic, MicOff, X } from "lucide-react";

import { Button } from "@/shared/ui/button";

/**
 * The overlay shown while a "Teach a task" recording is in progress —
 * a top bar over the stage so it never covers the screen the human is
 * demonstrating on. Purely presentational: `SandboxViewerDialog` owns the
 * recording lifecycle (start/stop broker calls, mic capture, transcription,
 * upload, and the message to the agent) and passes down only what this
 * banner needs to render and the two exits (Done, Cancel).
 *
 * Narration is spoken, not typed — there's no text input here. `listening`
 * reflects whether the mic tap is live; when the mic failed to start (denied
 * permission, no device), the banner still shows so the human can finish the
 * silent video recording instead of losing the whole flow.
 */
export function TeachTaskBanner({
  listening,
  onDone,
  onCancel,
  busy,
}: {
  listening: boolean;
  onDone: () => void;
  onCancel: () => void;
  busy: boolean;
}) {
  return (
    <div
      data-testid="teach-task-banner"
      className="absolute inset-x-3 top-3 z-10 flex flex-col gap-2 rounded-lg border border-red-500/30 bg-background/95 p-3 shadow-lg backdrop-blur-sm"
    >
      <div className="flex items-center gap-2">
        <Circle className="h-2.5 w-2.5 shrink-0 fill-red-500 text-red-500" />
        <span className="text-sm font-medium text-foreground">
          Recording — show me the task, then click Done
        </span>
      </div>

      <div
        data-testid="teach-task-listening"
        className="flex items-center gap-1.5 text-2xs text-muted-foreground"
      >
        {listening ? (
          <>
            <Mic className="h-3 w-3 shrink-0 animate-pulse text-red-500" />
            <span>Listening — narrate as you demonstrate</span>
          </>
        ) : (
          <>
            <MicOff className="h-3 w-3 shrink-0" />
            <span>No microphone — recording video only</span>
          </>
        )}
      </div>

      <div className="flex items-center justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          data-testid="teach-task-cancel"
          onClick={onCancel}
          disabled={busy}
          className="gap-1.5 text-2xs"
        >
          <X className="h-3.5 w-3.5" />
          Cancel
        </Button>
        <Button
          type="button"
          variant="default"
          size="sm"
          data-testid="teach-task-done"
          onClick={onDone}
          disabled={busy}
          className="text-2xs"
        >
          {busy ? "Finishing…" : "Done"}
        </Button>
      </div>
    </div>
  );
}
