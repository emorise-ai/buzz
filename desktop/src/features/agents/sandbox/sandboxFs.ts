import { invokeTauri } from "@/shared/api/tauri";

/** One entry returned by the broker's directory listing. */
export type FsEntry = {
  name: string;
  kind: "file" | "dir" | "other";
  size: number;
  mtime: number;
};

export type FsListing = {
  path: string;
  entries: FsEntry[];
};

/**
 * Sandbox file browser calls — thin wrappers over the Tauri commands in
 * `sandbox_viewer.rs`, which NIP-98-sign every request the same way the
 * lifecycle calls do. Every path must be absolute under `/workspace` or
 * `/home/agent`; the broker enforces that boundary.
 */

export async function listSandboxDir(
  sandboxId: string,
  path: string,
): Promise<FsListing> {
  return invokeTauri<FsListing>("sandbox_fs_list", { sandboxId, path });
}

/** Downloads the file to the user's Downloads folder; returns the saved path. */
export async function downloadSandboxFile(
  sandboxId: string,
  path: string,
): Promise<string> {
  return invokeTauri<string>("sandbox_fs_download", { sandboxId, path });
}

export async function uploadSandboxFile(
  sandboxId: string,
  localPath: string,
  destPath: string,
): Promise<void> {
  await invokeTauri("sandbox_fs_upload", { sandboxId, localPath, destPath });
}

export async function renameSandboxPath(
  sandboxId: string,
  from: string,
  to: string,
): Promise<void> {
  await invokeTauri("sandbox_fs_rename", { sandboxId, from, to });
}

export async function deleteSandboxPath(
  sandboxId: string,
  path: string,
): Promise<void> {
  await invokeTauri("sandbox_fs_delete", { sandboxId, path });
}
