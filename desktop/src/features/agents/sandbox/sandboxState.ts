import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_SANDBOX_CREATED,
  KIND_SANDBOX_DESTROYED,
} from "@/shared/constants/kinds";

/**
 * A live sandbox belonging to an agent — the "this agent has a computer" state
 * reconstructed from kind:48200 (created) / kind:48201 (destroyed) events.
 *
 * Tag contract is set by the broker (`crates/buzz-sandbox-broker/src/events.rs`):
 * 48200 carries `d`, `name`, `image`, `cpus`, `memory_mb`, `expires_at`, `p`
 * (owner), and an optional `viewer` URL for the live screen. 48201 carries `d`,
 * `reason` (`expired` | `destroyed`), and `p`.
 */
export type AgentSandbox = {
  /** Sandbox id — the `d` tag; the addressable key that 48201 references. */
  id: string;
  /** Container name, e.g. `buzz-sandbox-fq5ijxh7lt`. */
  name: string | null;
  /** Image the sandbox runs. */
  image: string | null;
  /** Enforced CPU budget. */
  cpus: number | null;
  /** Enforced memory budget, in MiB. */
  memoryMb: number | null;
  /** Unix seconds when the reaper destroys the sandbox. */
  expiresAt: number | null;
  /** URL of the live desktop, when the broker exposes one. Empty until the
   *  viewer is routed and `BUZZ_SANDBOX_VIEWER_URL` is set on the broker. */
  viewerUrl: string | null;
  /** created_at of the 48200 that announced it, for ordering. */
  createdAt: number;
};

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function optionalNumber(value: unknown): number | null {
  if (typeof value !== "number" && typeof value !== "string") return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

/**
 * Turn the broker's successful create/reuse response into immediate UI state.
 * The relay announcement remains the durable authority, but rendering must not
 * depend on that second network round-trip: an already-running reused machine
 * may have no fresh 48200 to replay at all.
 */
export function sandboxFromCreateResponse(
  value: unknown,
  createdAt = Math.floor(Date.now() / 1000),
): AgentSandbox | null {
  if (!value || typeof value !== "object") return null;
  const field = (name: string): unknown => Reflect.get(value, name);
  const id = field("id");
  if (typeof id !== "string" || id.length === 0) return null;

  return {
    id,
    name: optionalString(field("name")),
    image: optionalString(field("image")),
    cpus: optionalNumber(field("cpus")),
    memoryMb: optionalNumber(field("memory_mb")),
    expiresAt: optionalNumber(field("expires_at")),
    viewerUrl: optionalString(field("viewer_url")),
    createdAt,
  };
}

function tag(event: RelayEvent, name: string): string | null {
  const found = event.tags.find((t) => t[0] === name);
  return found?.[1] ?? null;
}

function num(value: string | null): number | null {
  if (value === null) return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

/**
 * Reconstruct an agent's current sandbox from the full set of seen lifecycle
 * events. Pure and order-independent: events may arrive out of order, be
 * replayed on reconnect, or land in same-second batches, so state is rebuilt
 * from scratch each time rather than mutated incrementally (the huddle pattern).
 *
 * Returns the single most-recent live sandbox, or null if none is live. A
 * sandbox is live when a 48200 exists for its `d` with no matching 48201.
 * (An agent has at most one sandbox today; if that ever changes, the newest
 * live one wins.)
 */
export function reconstructAgentSandbox(
  events: Iterable<RelayEvent>,
): AgentSandbox | null {
  const created = new Map<string, AgentSandbox>();
  const destroyed = new Set<string>();

  for (const event of events) {
    const id = tag(event, "d");
    if (!id) continue;

    if (event.kind === KIND_SANDBOX_CREATED) {
      // A later 48200 for the same id (e.g. a re-announcement) wins.
      const existing = created.get(id);
      if (existing && existing.createdAt > event.created_at) continue;
      created.set(id, {
        id,
        name: tag(event, "name"),
        image: tag(event, "image"),
        cpus: num(tag(event, "cpus")),
        memoryMb: num(tag(event, "memory_mb")),
        expiresAt: num(tag(event, "expires_at")),
        viewerUrl: tag(event, "viewer"),
        createdAt: event.created_at,
      });
    } else if (event.kind === KIND_SANDBOX_DESTROYED) {
      destroyed.add(id);
    }
  }

  let live: AgentSandbox | null = null;
  for (const sandbox of created.values()) {
    if (destroyed.has(sandbox.id)) continue;
    if (!live || sandbox.createdAt > live.createdAt) live = sandbox;
  }
  return live;
}
