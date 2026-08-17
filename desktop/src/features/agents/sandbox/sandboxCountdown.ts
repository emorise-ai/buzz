/**
 * Format the time remaining until a sandbox's `expires_at` (unix seconds) as a
 * short human countdown, given the current time in ms.
 *
 * Coarser than `formatElapsed`: expiry is minutes-to-hours away, so seconds are
 * only shown in the final minute. Returns `"expired"` once the deadline passes —
 * the reaper destroys the sandbox at that point and a 48201 will follow, but the
 * card should read as expired immediately rather than counting into the negative.
 */
export function formatSandboxRemaining(
  expiresAtSec: number,
  nowMs: number,
): string {
  const remainingSec = Math.floor(expiresAtSec - nowMs / 1000);
  if (remainingSec <= 0) return "expired";

  if (remainingSec < 60) return `${remainingSec}s left`;

  const totalMinutes = Math.floor(remainingSec / 60);
  if (totalMinutes < 60) return `${totalMinutes}m left`;

  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return minutes > 0 ? `${hours}h ${minutes}m left` : `${hours}h left`;
}

/** True once the deadline has passed. */
export function isSandboxExpired(expiresAtSec: number, nowMs: number): boolean {
  return expiresAtSec - nowMs / 1000 <= 0;
}
