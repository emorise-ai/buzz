import * as React from "react";

/** Whether "Teach a task" captures narration audio alongside the screen
 *  recording, or records video only. */
export type TeachTaskMode = "audio-video" | "video-only";

export const TEACH_TASK_MODE_STORAGE_KEY = "buzz.sandbox.teachTaskMode";
export const DEFAULT_TEACH_TASK_MODE: TeachTaskMode = "audio-video";

const listeners = new Set<() => void>();
let teachTaskMode = readStoredTeachTaskMode();

export function parseTeachTaskMode(
  value: string | null | undefined,
): TeachTaskMode {
  return value === "video-only" || value === "audio-video"
    ? value
    : DEFAULT_TEACH_TASK_MODE;
}

function readStoredTeachTaskMode(): TeachTaskMode {
  try {
    return parseTeachTaskMode(
      globalThis.localStorage?.getItem(TEACH_TASK_MODE_STORAGE_KEY),
    );
  } catch {
    return DEFAULT_TEACH_TASK_MODE;
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getTeachTaskMode(): TeachTaskMode {
  return teachTaskMode;
}

export function setTeachTaskMode(mode: TeachTaskMode): void {
  teachTaskMode = mode;
  try {
    globalThis.localStorage?.setItem(TEACH_TASK_MODE_STORAGE_KEY, mode);
  } catch {
    // Persistence is best-effort; the in-memory preference still applies.
  }
  for (const listener of listeners) listener();
}

/** Reactive last-used "Teach a task" recording mode, persisted across
 *  sessions so the dropdown remembers the human's choice. */
export function useTeachTaskMode(): TeachTaskMode {
  return React.useSyncExternalStore(
    subscribe,
    getTeachTaskMode,
    () => DEFAULT_TEACH_TASK_MODE,
  );
}
