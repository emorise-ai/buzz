import { Circle, ChevronDown, GraduationCap, X } from "lucide-react";

import { Button } from "@/shared/ui/button";
import { cn } from "@/shared/lib/cn";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import type { TeachTaskMode } from "./teachTaskModePreference";

/**
 * The "Teach a task" control shared by `SandboxViewerDialog` and
 * `ComputerWindowScreen` — one button that both starts and finishes a
 * recording (no separate overlay/banner; the button itself is the
 * recording indicator) plus a small dropdown to pick Audio+Video vs Video
 * only and, while recording, to discard instead of sending.
 *
 * Takes the relevant slice of `useTeachTask`'s return as props rather than
 * calling the hook itself — the hook's unmount cleanup (stopping an orphaned
 * sandbox recording) needs to be tied to the host surface's own lifecycle,
 * so each surface still owns its own `useTeachTask()` call.
 */
export function TeachTaskButton({
  teaching,
  mode,
  setMode,
  onStart,
  onFinish,
  onCancel,
}: {
  teaching: boolean;
  mode: TeachTaskMode;
  setMode: (mode: TeachTaskMode) => void;
  onStart: (mode: TeachTaskMode) => void;
  onFinish: () => void;
  onCancel: () => void;
}) {
  if (teaching) {
    // Click either control returns to idle immediately — finishing/discarding
    // both hand off to background work (see `useTeachTask`), so there's no
    // in-between "Finishing…" state to show or to disable against.
    return (
      <div className="flex items-center gap-1">
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="sandbox-teach-task"
          onClick={onFinish}
          className={cn(
            "gap-1.5 border-red-500 text-2xs text-red-500 hover:bg-red-500/10 hover:text-red-500",
          )}
        >
          <Circle className="h-2.5 w-2.5 fill-red-500 text-red-500 animate-pulse" />
          Recording…
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label="Discard recording"
          data-testid="sandbox-teach-task-discard"
          onClick={onCancel}
          className="text-muted-foreground hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>
    );
  }

  return (
    <div className="flex items-center">
      <Button
        type="button"
        variant="outline"
        size="sm"
        data-testid="sandbox-teach-task"
        onClick={() => onStart(mode)}
        className="gap-1.5 rounded-r-none border-r-0 text-2xs"
      >
        <GraduationCap className="h-3.5 w-3.5" />
        Teach a task
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            aria-label="Recording options"
            data-testid="sandbox-teach-task-options"
            className="h-8 w-6 rounded-l-none px-0"
          >
            <ChevronDown className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-48">
          <DropdownMenuCheckboxItem
            checked={mode === "audio-video"}
            onCheckedChange={() => setMode("audio-video")}
          >
            Audio + Video
          </DropdownMenuCheckboxItem>
          <DropdownMenuCheckboxItem
            checked={mode === "video-only"}
            onCheckedChange={() => setMode("video-only")}
          >
            Video only
          </DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
