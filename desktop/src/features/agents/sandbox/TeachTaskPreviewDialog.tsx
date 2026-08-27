import { Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/shared/ui/dialog";
import { Button } from "@/shared/ui/button";
import type { TeachTaskPreview } from "./useTeachTask";

/**
 * Preview-before-send for "Teach a task." Opens once `doneTeaching` has
 * stopped the sandbox recording and the local mp4 bytes are ready — before
 * anything is uploaded or sent to the agent. The human watches (and hears)
 * exactly what was captured, then either confirms (`onSend`, which uploads
 * and messages the agent's DM) or discards (`onDiscard`, which just releases
 * the local blob — the sandbox recording was already stopped, so nothing
 * keeps running).
 *
 * Rendered by each surface that owns a `useTeachTask()` instance
 * (`SandboxViewerDialog`, `ComputerWindowScreen`) rather than lifted into a
 * shared singleton, since each surface already has its own hook instance.
 */
export function TeachTaskPreviewDialog({
  preview,
  onSend,
  onDiscard,
}: {
  preview: TeachTaskPreview | null;
  onSend: () => void;
  onDiscard: () => void;
}) {
  const narrationText = preview?.transcript.trim() ?? "";

  return (
    <Dialog
      open={preview !== null}
      onOpenChange={(open) => {
        if (!open) onDiscard();
      }}
    >
      <DialogContent
        data-testid="teach-task-preview"
        className="max-w-xl gap-4"
      >
        <DialogHeader>
          <DialogTitle>Review before sending</DialogTitle>
          <DialogDescription>
            Watch what was captured, then send it to the agent or discard it.
          </DialogDescription>
        </DialogHeader>

        {preview ? (
          <video
            key={preview.videoUrl}
            src={preview.videoUrl}
            controls
            autoPlay
            muted={false}
            className="max-h-[50vh] w-full rounded-lg bg-black"
          />
        ) : null}

        <div className="rounded-lg border border-border bg-muted/40 p-3">
          {narrationText ? (
            <p className="text-sm text-foreground">
              <span className="font-medium">What I heard:</span> {narrationText}
            </p>
          ) : preview?.transcribing ? (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Transcribing your narration…
            </p>
          ) : (
            <p className="text-sm text-muted-foreground">
              No narration was transcribed.
            </p>
          )}
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            data-testid="teach-task-preview-discard"
            onClick={onDiscard}
          >
            Discard
          </Button>
          <Button
            type="button"
            data-testid="teach-task-preview-send"
            onClick={onSend}
          >
            Send to agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
