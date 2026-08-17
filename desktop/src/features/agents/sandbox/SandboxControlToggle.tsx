import { MousePointerClick, User } from "lucide-react";

import { cn } from "@/shared/lib/cn";

/**
 * "Agent is controlling — click to take over" / "You're controlling — hand
 * back". Pure UI state local to the workspace: this component only renders
 * the indicator; the caller owns the boolean and the overlay that actually
 * blocks pointer events on the screen views.
 */
export function SandboxControlToggle({
  userInControl,
  onToggle,
}: {
  userInControl: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      data-testid="sandbox-control-toggle"
      onClick={onToggle}
      className={cn(
        "flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-2xs font-medium transition-colors",
        userInControl
          ? "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400"
          : "border-border bg-muted/40 text-muted-foreground hover:bg-muted/70",
      )}
    >
      {userInControl ? (
        <User className="h-3 w-3 shrink-0" />
      ) : (
        <MousePointerClick className="h-3 w-3 shrink-0" />
      )}
      {userInControl
        ? "You're controlling — hand back"
        : "Agent is controlling — click to take over"}
    </button>
  );
}
