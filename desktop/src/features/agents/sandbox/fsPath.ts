/**
 * Pure path helpers for the sandbox file browser. The broker only accepts
 * absolute paths under `/workspace` or `/home/agent`; these helpers keep the
 * UI's breadcrumb/navigation math independent of any network call.
 */

/** The two roots the Files view can switch between. */
export const FS_ROOTS = ["/workspace", "/home/agent"] as const;
export type FsRoot = (typeof FS_ROOTS)[number];

/** Join a directory path and a child name into a normalized absolute path. */
export function joinPath(dir: string, name: string): string {
  const trimmedDir = dir.endsWith("/") ? dir.slice(0, -1) : dir;
  return `${trimmedDir}/${name}`;
}

/** The parent of a path, clamped at `root` — never rises above it. */
export function parentPath(path: string, root: string): string {
  if (path === root) return root;
  const idx = path.lastIndexOf("/");
  const parent = idx <= 0 ? "/" : path.slice(0, idx);
  return parent.length < root.length || !path.startsWith(root) ? root : parent;
}

/** Breadcrumb segments from `root` down to `path`, each with its full path. */
export function breadcrumbSegments(
  path: string,
  root: string,
): { label: string; path: string }[] {
  const rootLabel = root.split("/").filter(Boolean).pop() ?? root;
  const segments: { label: string; path: string }[] = [
    { label: rootLabel, path: root },
  ];
  if (path === root || !path.startsWith(root)) return segments;

  const rest = path.slice(root.length).split("/").filter(Boolean);
  let current = root;
  for (const part of rest) {
    current = joinPath(current, part);
    segments.push({ label: part, path: current });
  }
  return segments;
}
