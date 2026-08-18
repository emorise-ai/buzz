import type * as React from "react";

import { cn } from "@/shared/lib/cn";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import type { LaunchableApp } from "./sandboxLaunch";
import type { SandboxWindow } from "./sandboxWindows";
import {
  BrowserDockIcon,
  ComputerDockIcon,
  FilesDockIcon,
  TerminalDockIcon,
} from "./sandboxDockIcons";

const LAUNCH_ITEMS: {
  app: LaunchableApp;
  label: string;
  Icon: React.ComponentType;
}[] = [
  { app: "browser", label: "Browser", Icon: BrowserDockIcon },
  { app: "files", label: "Files", Icon: FilesDockIcon },
  { app: "terminal", label: "Terminal", Icon: TerminalDockIcon },
];

/** One taskbar chip: every open window of one app, grouped the way an OS
 *  taskbar stacks them. The chip carries the dock's own artwork for that
 *  app; unknown apps share the generic computer glyph. */
type WindowGroup = {
  kind: "browser" | "files" | "terminal" | "other";
  label: string;
  Icon: React.ComponentType;
  windows: SandboxWindow[];
};

function groupKindFor(
  wmClass: string,
): Pick<WindowGroup, "kind" | "label" | "Icon"> {
  if (wmClass.includes("chromium") || wmClass.includes("chrome")) {
    return { kind: "browser", label: "Browser", Icon: BrowserDockIcon };
  }
  if (wmClass.includes("thunar")) {
    return { kind: "files", label: "Files", Icon: FilesDockIcon };
  }
  if (wmClass.includes("terminal")) {
    return { kind: "terminal", label: "Terminal", Icon: TerminalDockIcon };
  }
  return { kind: "other", label: "App", Icon: ComputerDockIcon };
}

function groupWindows(windows: SandboxWindow[]): WindowGroup[] {
  const groups = new Map<WindowGroup["kind"], WindowGroup>();
  for (const w of windows) {
    const kind = groupKindFor(w.wmClass);
    const group = groups.get(kind.kind) ?? { ...kind, windows: [] };
    group.windows.push(w);
    groups.set(kind.kind, group);
  }
  return [...groups.values()];
}

/**
 * The floating dock over the sandbox's screen. There is only one stage now —
 * the live desktop — so this is no longer a view switcher. Browser, Files,
 * and Terminal each open a real window on that desktop (see
 * `sandboxLaunch.ts`); Computer is "just look at the desktop," the
 * always-on default the dot marks, with no launch action of its own.
 *
 * Real app-style icons (see `sandboxDockIcons.tsx`) sit on a translucent
 * blurred pill detached from the stage, so it reads as a dock floating over
 * the screen rather than a full-width toolbar strip.
 *
 * `compact` shrinks the icons and drops the taskbar's text labels (icons and
 * counts only) for the sidebar preview panel, which is roughly 300px wide —
 * the full-size dock's labeled taskbar chips overflow that width. The
 * fullscreen dialog and pop-out window keep the full-size dock.
 */
export function SandboxDock({
  launching,
  onLaunch,
  windows,
  onWindowClick,
  compact = false,
}: {
  /** The app currently mid-launch, for a per-icon busy state, or null. */
  launching: LaunchableApp | null;
  onLaunch: (app: LaunchableApp) => void;
  /** Open windows on the desktop, shown as a taskbar segment after the
   *  launchers — click focuses a window, click the focused one to minimize. */
  windows: SandboxWindow[];
  onWindowClick: (window: SandboxWindow) => void;
  /** Narrow-container mode — see doc comment above. Defaults false. */
  compact?: boolean;
}) {
  const iconBoxClass = compact ? "h-7 w-7" : "h-11 w-11";
  const groupIconBoxClass = compact ? "h-4 w-4" : "h-6 w-6";

  return (
    <div
      className={cn("flex justify-center", compact ? "pb-1.5" : "pb-3 pt-1")}
    >
      <div
        className={cn(
          "flex items-end rounded-2xl border border-border/60 bg-background/70 shadow-lg backdrop-blur-md",
          compact ? "gap-1.5 px-1.5 py-1" : "gap-3 px-3 py-2",
        )}
      >
        {LAUNCH_ITEMS.map(({ app, label, Icon }) => {
          const isLaunching = launching === app;
          return (
            <Tooltip key={app}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  data-testid={`sandbox-dock-${app}`}
                  aria-label={`Open ${label}`}
                  aria-pressed={isLaunching}
                  disabled={launching !== null}
                  onClick={() => onLaunch(app)}
                  className={cn(
                    "group flex flex-col items-center rounded-xl outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-default",
                    compact ? "gap-1" : "gap-1.5",
                  )}
                >
                  <span
                    className={cn(
                      "block drop-shadow-lg transition-all duration-150 ease-out motion-reduce:transition-none",
                      iconBoxClass,
                      "group-active:scale-95 motion-reduce:group-active:scale-100",
                      isLaunching
                        ? "scale-95 opacity-100 saturate-100"
                        : "opacity-75 saturate-75 group-enabled:group-hover:-translate-y-1 group-enabled:group-hover:scale-110 group-enabled:group-hover:opacity-100 group-enabled:group-hover:saturate-100 motion-reduce:group-hover:translate-y-0 motion-reduce:group-hover:scale-100",
                    )}
                  >
                    <Icon />
                  </span>
                  {/* Momentary pressed/launched feedback, not a persistent
                      active state — these are launchers, not view tabs. */}
                  <span
                    className={cn(
                      "h-1 w-1 rounded-full transition-opacity duration-150 motion-reduce:transition-none",
                      isLaunching ? "bg-foreground opacity-70" : "opacity-0",
                    )}
                  />
                </button>
              </TooltipTrigger>
              <TooltipContent side="top">
                {isLaunching ? `Opening ${label}…` : `Open ${label}`}
              </TooltipContent>
            </Tooltip>
          );
        })}

        {/* Computer: the always-visible desktop, not a launcher. The dot
            marks it as the permanent active canvas. */}
        <Tooltip>
          <TooltipTrigger asChild>
            <span
              role="status"
              data-testid="sandbox-dock-computer"
              aria-label="Computer — active"
              className={cn(
                "flex flex-col items-center rounded-xl",
                compact ? "gap-1" : "gap-1.5",
              )}
            >
              <span
                className={cn(
                  "block opacity-100 drop-shadow-lg saturate-100",
                  iconBoxClass,
                )}
              >
                <ComputerDockIcon />
              </span>
              <span className="h-1 w-1 rounded-full bg-foreground opacity-70" />
            </span>
          </TooltipTrigger>
          <TooltipContent side="top">Computer</TooltipContent>
        </Tooltip>

        {/* Taskbar segment: the desktop's open windows, straight from the
            window manager via the broker. Click focuses a window; clicking
            the focused one minimizes it — the running-windows half of a
            Windows-style taskbar, sharing the dock pill with the launchers
            so there is exactly one strip of chrome on screen. Compact mode
            drops the title text (icon + count only) so chips stay narrow
            enough for the sidebar panel. */}
        {windows.length > 0 ? (
          <>
            <span
              aria-hidden="true"
              className={cn(
                "self-end bg-border/60",
                compact ? "mb-1 h-6 w-px" : "mb-1.5 h-10 w-px",
              )}
            />
            <div
              className={cn(
                "flex min-w-0 items-center self-end overflow-x-auto",
                compact
                  ? "mb-1 max-w-40 gap-1"
                  : "mb-1.5 max-w-[52rem] gap-1.5",
              )}
            >
              {groupWindows(windows).map((group) => {
                const { Icon: GroupIcon } = group;
                const activeInGroup = group.windows.some((w) => w.active);
                if (group.windows.length === 1) {
                  const w = group.windows[0];
                  return (
                    <Tooltip key={group.kind}>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          data-testid={`sandbox-dock-window-${w.id}`}
                          aria-label={
                            w.active
                              ? `Minimize ${w.title}`
                              : `Focus ${w.title}`
                          }
                          aria-pressed={w.active}
                          onClick={() => onWindowClick(w)}
                          className={cn(
                            "flex shrink-0 items-center rounded-xl border outline-hidden transition-colors duration-150",
                            "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
                            compact
                              ? "h-7 w-7 justify-center px-0 py-0"
                              : "max-w-44 gap-2 px-2.5 py-2",
                            w.active
                              ? "border-border/60 bg-foreground/15"
                              : "border-transparent hover:bg-foreground/10",
                          )}
                        >
                          <span
                            className={cn(
                              "block shrink-0 drop-shadow-md",
                              groupIconBoxClass,
                            )}
                          >
                            <GroupIcon />
                          </span>
                          {!compact ? (
                            <span
                              className={cn(
                                "truncate text-xs",
                                w.active
                                  ? "text-foreground"
                                  : "text-foreground/70",
                              )}
                            >
                              {w.title}
                            </span>
                          ) : null}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent side="top">
                        {w.active ? `Minimize ${w.title}` : w.title}
                      </TooltipContent>
                    </Tooltip>
                  );
                }
                // Several windows of the same app collapse into one chip
                // with a count; clicking opens the window list, like an OS
                // taskbar group.
                return (
                  <DropdownMenu key={group.kind}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <DropdownMenuTrigger asChild>
                          <button
                            type="button"
                            data-testid={`sandbox-dock-group-${group.kind}`}
                            aria-label={`${group.label}: ${group.windows.length} windows`}
                            className={cn(
                              "relative flex shrink-0 items-center rounded-xl border outline-hidden transition-colors duration-150",
                              "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
                              compact
                                ? "gap-1 px-1.5 py-1"
                                : "gap-2 px-2.5 py-2",
                              activeInGroup
                                ? "border-border/60 bg-foreground/15"
                                : "border-transparent hover:bg-foreground/10",
                            )}
                          >
                            <span
                              className={cn(
                                "block shrink-0 drop-shadow-md",
                                groupIconBoxClass,
                              )}
                            >
                              <GroupIcon />
                            </span>
                            <span
                              className={cn(
                                "min-w-4 rounded-full bg-foreground/15 px-1.5 py-0.5 text-center text-2xs font-medium tabular-nums",
                                activeInGroup
                                  ? "text-foreground"
                                  : "text-foreground/70",
                              )}
                            >
                              {group.windows.length}
                            </span>
                          </button>
                        </DropdownMenuTrigger>
                      </TooltipTrigger>
                      <TooltipContent side="top">
                        {group.label} — {group.windows.length} windows
                      </TooltipContent>
                    </Tooltip>
                    <DropdownMenuContent side="top" align="center">
                      {group.windows.map((w) => (
                        <DropdownMenuItem
                          key={w.id}
                          data-testid={`sandbox-dock-window-${w.id}`}
                          onSelect={() => onWindowClick(w)}
                          className="max-w-72 gap-2"
                        >
                          <span
                            className={cn(
                              "h-1.5 w-1.5 shrink-0 rounded-full",
                              w.active ? "bg-foreground" : "bg-foreground/25",
                            )}
                          />
                          <span className="truncate">{w.title}</span>
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuContent>
                  </DropdownMenu>
                );
              })}
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
