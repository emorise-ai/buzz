import { Circle, X } from "lucide-react";

import { Button } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

/**
 * The overlay shown while a "Teach a task" recording is in progress —
 * a top bar over the stage so it never covers the screen the human is
 * demonstrating on. Purely presentational: `SandboxViewerDialog` owns the
 * recording lifecycle (start/stop broker calls, upload, and the message to
 * the agent) and passes down only what this banner needs to render and the
 * two exits (Done, Cancel).
 */
export function TeachTaskBanner({
  narration,
  onNarrationChange,
  onDone,
  onCancel,
  busy,
}: {
  narration: string;
  onNarrationChange: (value: string) => void;
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

      <Textarea
        data-testid="teach-task-narration"
        value={narration}
        onChange={(e) => onNarrationChange(e.target.value)}
        placeholder="Narrate what you're doing (optional)…"
        disabled={busy}
        className="min-h-14 text-sm"
      />

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
