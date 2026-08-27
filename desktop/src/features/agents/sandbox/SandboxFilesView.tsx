import * as React from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronRight,
  Download,
  File,
  Folder,
  Loader2,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react";
import { toast } from "sonner";

import { cn } from "@/shared/lib/cn";
import { useNow } from "@/shared/lib/useNow";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { FS_ROOTS, breadcrumbSegments, joinPath, parentPath } from "./fsPath";
import { formatFileSize, formatRelativeMtime } from "./fsFormat";
import {
  type FsEntry,
  deleteSandboxPath,
  downloadSandboxFile,
  listSandboxDir,
  renameSandboxPath,
  uploadSandboxFile,
} from "./sandboxFs";

type SortMode = "name" | "recent";

/**
 * The Files view: a browser over the sandbox's `/workspace` and `/home/agent`
 * trees. Every mutation (rename/delete/upload) re-lists the current directory
 * on success rather than patching local state — the broker is the source of
 * truth and listings are cheap.
 */
export function SandboxFilesView({ sandboxId }: { sandboxId: string }) {
  const [root, setRoot] = React.useState<(typeof FS_ROOTS)[number]>(
    FS_ROOTS[0],
  );
  const [path, setPath] = React.useState<string>(FS_ROOTS[0]);
  const [entries, setEntries] = React.useState<FsEntry[] | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [sortMode, setSortMode] = React.useState<SortMode>("name");
  const [uploading, setUploading] = React.useState(false);
  const [renaming, setRenaming] = React.useState<FsEntry | null>(null);
  const [renameValue, setRenameValue] = React.useState("");
  const [deleting, setDeleting] = React.useState<FsEntry | null>(null);
  const [busyName, setBusyName] = React.useState<string | null>(null);
  const now = useNow(60_000);

  const load = React.useCallback(
    async (targetPath: string) => {
      setLoading(true);
      setError(null);
      try {
        const listing = await listSandboxDir(sandboxId, targetPath);
        setEntries(listing.entries);
        setPath(listing.path);
      } catch (err) {
        setEntries(null);
        setError(
          err instanceof Error ? err.message : "Could not list this folder.",
        );
      } finally {
        setLoading(false);
      }
    },
    [sandboxId],
  );

  React.useEffect(() => {
    void load(root);
  }, [root, load]);

  const sorted = React.useMemo(() => {
    if (!entries) return [];
    const copy = [...entries];
    if (sortMode === "recent") {
      copy.sort((a, b) => b.mtime - a.mtime);
    } else {
      copy.sort((a, b) => {
        if (a.kind === "dir" && b.kind !== "dir") return -1;
        if (a.kind !== "dir" && b.kind === "dir") return 1;
        return a.name.localeCompare(b.name);
      });
    }
    return copy;
  }, [entries, sortMode]);

  async function handleUpload() {
    const selected = await openFileDialog({
      multiple: false,
      directory: false,
    });
    if (!selected || Array.isArray(selected)) return;
    const fileName = selected.split(/[/\\]/).pop() ?? "upload";
    const destPath = joinPath(path, fileName);
    setUploading(true);
    try {
      await uploadSandboxFile(sandboxId, selected, destPath);
      await load(path);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Upload failed.");
    } finally {
      setUploading(false);
    }
  }

  async function handleDownload(entry: FsEntry) {
    setBusyName(entry.name);
    try {
      const saved = await downloadSandboxFile(
        sandboxId,
        joinPath(path, entry.name),
      );
      toast.success(`Saved to ${saved}`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Download failed.");
    } finally {
      setBusyName(null);
    }
  }

  async function handleRenameConfirm() {
    if (!renaming) return;
    const trimmed = renameValue.trim();
    if (!trimmed || trimmed === renaming.name) {
      setRenaming(null);
      return;
    }
    setBusyName(renaming.name);
    try {
      await renameSandboxPath(
        sandboxId,
        joinPath(path, renaming.name),
        joinPath(path, trimmed),
      );
      await load(path);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Rename failed.");
    } finally {
      setBusyName(null);
      setRenaming(null);
    }
  }

  async function handleDeleteConfirm() {
    if (!deleting) return;
    setBusyName(deleting.name);
    try {
      await deleteSandboxPath(sandboxId, joinPath(path, deleting.name));
      await load(path);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Delete failed.");
    } finally {
      setBusyName(null);
      setDeleting(null);
    }
  }

  const crumbs = breadcrumbSegments(path, root);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <div className="flex items-center gap-1 rounded-md bg-muted/40 p-0.5 text-2xs">
          {FS_ROOTS.map((r) => (
            <button
              key={r}
              type="button"
              data-testid={`sandbox-fs-root-${r === "/workspace" ? "workspace" : "home"}`}
              onClick={() => setRoot(r)}
              className={cn(
                "rounded px-2 py-1 font-medium transition-colors",
                root === r
                  ? "bg-background text-foreground shadow-xs"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {r === "/workspace" ? "Workspace" : "Home"}
            </button>
          ))}
        </div>

        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto text-2xs text-muted-foreground">
          {crumbs.map((crumb, idx) => (
            <React.Fragment key={crumb.path}>
              {idx > 0 ? <ChevronRight className="h-3 w-3 shrink-0" /> : null}
              <button
                type="button"
                onClick={() => void load(crumb.path)}
                className={cn(
                  "shrink-0 rounded px-1 py-0.5 hover:bg-muted/50",
                  idx === crumbs.length - 1
                    ? "font-medium text-foreground"
                    : "hover:text-foreground",
                )}
              >
                {crumb.label}
              </button>
            </React.Fragment>
          ))}
        </div>

        <button
          type="button"
          data-testid="sandbox-fs-sort-recent"
          onClick={() =>
            setSortMode((m) => (m === "recent" ? "name" : "recent"))
          }
          className={cn(
            "shrink-0 rounded px-2 py-1 text-2xs font-medium transition-colors",
            sortMode === "recent"
              ? "bg-primary/10 text-primary"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          Recent
        </button>

        <button
          type="button"
          onClick={() => void load(path)}
          title="Refresh"
          className="shrink-0 rounded p-1 text-muted-foreground hover:bg-muted/50 hover:text-foreground"
        >
          <RefreshCw className="h-3.5 w-3.5" />
        </button>

        <Button
          type="button"
          size="sm"
          variant="outline"
          data-testid="sandbox-fs-upload"
          disabled={uploading}
          onClick={() => void handleUpload()}
          className="h-7 gap-1 text-2xs"
        >
          {uploading ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Upload className="h-3 w-3" />
          )}
          Upload
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex h-full items-center justify-center text-muted-foreground">
            <Loader2 className="h-5 w-5 animate-spin" />
          </div>
        ) : error ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
            <p className="text-sm font-medium text-foreground">
              Could not load this folder
            </p>
            <p className="max-w-sm text-2xs text-muted-foreground">{error}</p>
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => void load(path)}
              className="mt-1 h-7 text-2xs"
            >
              Try again
            </Button>
          </div>
        ) : sorted.length === 0 ? (
          <div className="flex h-full items-center justify-center text-2xs text-muted-foreground">
            This folder is empty.
          </div>
        ) : (
          <ul className="divide-y divide-border/60">
            {path !== root ? (
              <li>
                <button
                  type="button"
                  onClick={() => void load(parentPath(path, root))}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm text-muted-foreground hover:bg-muted/40"
                >
                  <Folder className="h-4 w-4 shrink-0" />
                  ..
                </button>
              </li>
            ) : null}
            {sorted.map((entry) => (
              <li
                key={entry.name}
                data-testid={`sandbox-fs-entry-${entry.name}`}
                className="group flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted/40"
              >
                <button
                  type="button"
                  onClick={() => {
                    if (entry.kind === "dir") {
                      void load(joinPath(path, entry.name));
                    }
                  }}
                  disabled={entry.kind !== "dir"}
                  className={cn(
                    "flex min-w-0 flex-1 items-center gap-2 text-left",
                    entry.kind === "dir" ? "cursor-pointer" : "cursor-default",
                  )}
                >
                  {entry.kind === "dir" ? (
                    <Folder className="h-4 w-4 shrink-0 text-muted-foreground" />
                  ) : (
                    <File className="h-4 w-4 shrink-0 text-muted-foreground" />
                  )}
                  <span className="truncate">{entry.name}</span>
                </button>

                <span className="w-16 shrink-0 text-right text-2xs text-muted-foreground">
                  {entry.kind === "file" ? formatFileSize(entry.size) : ""}
                </span>
                <span className="w-20 shrink-0 text-right text-2xs text-muted-foreground">
                  {formatRelativeMtime(entry.mtime, now)}
                </span>

                <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                  {busyName === entry.name ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
                  ) : (
                    <>
                      {entry.kind === "file" ? (
                        <button
                          type="button"
                          title="Download"
                          data-testid={`sandbox-fs-download-${entry.name}`}
                          onClick={() => void handleDownload(entry)}
                          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
                        >
                          <Download className="h-3.5 w-3.5" />
                        </button>
                      ) : null}
                      <button
                        type="button"
                        title="Rename"
                        data-testid={`sandbox-fs-rename-${entry.name}`}
                        onClick={() => {
                          setRenaming(entry);
                          setRenameValue(entry.name);
                        }}
                        className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </button>
                      <button
                        type="button"
                        title="Delete"
                        data-testid={`sandbox-fs-delete-${entry.name}`}
                        onClick={() => setDeleting(entry)}
                        className="rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <AlertDialog
        open={renaming !== null}
        onOpenChange={(open) => !open && setRenaming(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rename</AlertDialogTitle>
            <AlertDialogDescription>
              Choose a new name for &quot;{renaming?.name}&quot;.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <Input
            value={renameValue}
            onChange={(e) => setRenameValue(e.target.value)}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleRenameConfirm();
            }}
          />
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => setRenaming(null)}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction onClick={() => void handleRenameConfirm()}>
              Rename
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={deleting !== null}
        onOpenChange={(open) => !open && setDeleting(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Delete &quot;{deleting?.name}&quot;?
            </AlertDialogTitle>
            <AlertDialogDescription>
              {deleting?.kind === "dir"
                ? "This deletes the folder and everything inside it. This cannot be undone."
                : "This cannot be undone."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => setDeleting(null)}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() => void handleDeleteConfirm()}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
