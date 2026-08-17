/**
 * Formatting helpers for the sandbox file browser — human file sizes and
 * relative modification times. Pure so they can be unit tested without a
 * broker or a clock mock beyond passing `nowMs` explicitly.
 */

/** Format a byte count as a short human size (`"1.2 KB"`, `"4 MB"`). */
export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const digits = value < 10 ? 1 : 0;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
}

/** Format a unix-seconds mtime as a short relative label (`"3m ago"`). */
export function formatRelativeMtime(mtimeSec: number, nowMs: number): string {
  const diffSec = Math.max(0, Math.floor(nowMs / 1000 - mtimeSec));
  if (diffSec < 5) return "just now";
  if (diffSec < 60) return `${diffSec}s ago`;

  const minutes = Math.floor(diffSec / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;

  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;

  const years = Math.floor(months / 12);
  return `${years}y ago`;
}
