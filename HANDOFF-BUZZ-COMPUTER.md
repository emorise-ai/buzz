# Handoff — Buzz Computer, Phase 1 (the workspace)

**Written 2026-08-17.** One working day took the sandbox from "a noVNC iframe
in a dialog" to the Buzz Computer workspace: a real, Ubuntu-themed desktop the
user watches, takes over, and populates with windows from a dock — plus the
file API, web terminal, and app-launch plumbing behind it. Everything below is
**deployed to staging (Matterhorn) and verified live**, and as of the
evening of 2026-08-17 **committed** on `feat/agent-sandboxes` as four
commits (image, broker, cli, desktop) — plus same-day additions: Chromium-
matched window chrome, and a grouped taskbar segment in the dock fed by the
broker's new /windows API.

The product plan this implements: the "Buzz Computer + Teach a Task" artifact
(supersedes `SANDBOX-PLAN.md`, whose phases are all complete) —
https://claude.ai/code/artifact/75c64009-f5ba-4ec7-9c77-9f018aac129f

---

## What the user sees now

Open an agent's profile → Start a computer → open it:

- **One live screen** (the whole workspace is the desktop stream, 16:9-fitted,
  near-fullscreen dialog titled "*Agent*'s computer").
- **A dock floating on the desktop's bottom edge** (inside the stream's box,
  not below it). Icons: Chromium (official logo), Files, Terminal, Computer.
  Browser/Files/Terminal are **launchers** — each click opens another real
  window of that app on the desktop via the broker; Computer is a static
  "you're looking at it" indicator. Launching flips control to the user.
- **Top bar**: control indicator ("Agent is controlling — click to take over"
  / "You're controlling — hand back"), **Transfer files** (Mac ↔ computer,
  opens the file-browser overlay), and a disabled **Teach a task** button
  ("Coming soon" — the feature is Phase 4 of the plan; note the name, NOT
  "Teach Me").
- **Inside the computer**: Ubuntu Yaru theme (icons + GTK dark), Thunar file
  manager, xfce4-terminal (JetBrains Mono), Chromium as a large decorated
  window, navy-teal wallpaper, aubergine-tinted flat titlebars (BuzzDark
  openbox theme), picom shadows/fades/focus-dim, Alt+Tab, styled right-click
  launcher menu. Windows drag/resize/overlap normally (proven with scripted
  drags).

## The moving parts (all in the working tree)

| Piece | Where | State |
|---|---|---|
| Broker: terminal proxy `/sandboxes/{id}/terminal[...]` → ttyd :7681 | `crates/buzz-sandbox-broker` | deployed, verified live |
| Broker: fs API `/sandboxes/{id}/fs*` (list/download/upload/rename/delete, `/workspace` + `/home/agent` only, docker exec/archive, uid 10001) | same | deployed, full cycle verified live |
| Broker: `POST /sandboxes/{id}/launch` `{app: browser\|files\|terminal}` → fixed argv, detached exec | same | deployed, verified live (incl. injection rejection) |
| Desktop image: ttyd, Thunar, xfce4-terminal, Yaru, BuzzDark, picom, wallpaper, fonts, menu.xml, browser wrapper | `Dockerfile.sprig-desktop`, `scripts/sprig-desktop-*` | built on Matterhorn as `buzz-sprig-desktop:latest` |
| Tauri commands: `sandbox_fs_*` (5), `sandbox_launch_app`, existing mint/create/destroy | `desktop/src-tauri/src/sandbox_viewer.rs` | gates green |
| Workspace UI: dock-on-desktop, launchers, transfer overlay, control toggle, 16:9 stage fit | `desktop/src/features/agents/sandbox/` | gates green, hot-tested by user |
| Broker tests | — | 61 pass, clippy -D warnings clean |
| Desktop tests | — | 39 pass; tsc, biome, check:px-text clean |

## Hard-won facts (do not relearn these)

1. **NIP-98 on query-bearing routes must verify the RAW percent-encoded query**
   (axum `RawQuery`), never the decoded `Query` value — the app signs the URL
   exactly as sent (`path=%2Fworkspace`). Regression-tested in
   `identity.rs`. Decoded reconstruction 401s everything.
2. **axum `{*rest}` wildcards do not match an empty trailing segment.** The
   terminal route needs the explicit literal `/t/{token}/` route registered
   alongside the wildcard, and the redirect MUST keep the trailing-slash form —
   ttyd derives its WebSocket URL from `location.pathname`, so a `/view`-style
   synthetic tail breaks the socket. Router-level tests pin both.
3. **Google Chrome cannot run in the hardened sandbox** (cap-drop ALL +
   no-new-privileges): crashpad SIGTRAPs at startup; every flag and even
   `--cap-add SYS_PTRACE` fails. Chromium (same engine) works. `buzz-browser`
   wrapper prefers chromium; full test record in `Dockerfile.sprig-desktop`.
4. **An openbox `<mouse>` or `<keyboard>` block replaces ALL compiled-in
   bindings** — defining only your new binding silently deletes titlebar
   dragging / Alt+Tab. rc.xml restates the defaults.
5. **Openbox reads titlebar/menu fonts from rc.xml `<font place=…>`, not the
   themerc** — a themerc font key is silently ignored and titlebars collapse.
6. **pcmanfm links GTK2 on bookworm** (dated no matter the theme) — that's why
   Thunar. `~/.gtkrc-2.0` stays for any future GTK2 stray.
7. **The stage-measuring hook needs a state-backed callback ref** — dialog
   content mounts on open; a plain RefObject leaves the screen permanently
   `visibility: hidden`.
8. **The Terminal derivation must strip `?t=…`** — the dialog holds the
   *minted* viewer URL; deriving from it without stripping the query broke the
   Terminal view ("does not expose a terminal").
9. Judging desktop screenshots: **crop to the window before trusting colors** —
   downscaled 1920x1080 composites made dark UIs look washed-out twice.
10. Deploy mechanics: rsync tree → `/opt/buzz-sandbox-broker`, regenerate
    `Cargo.lock` in a rust container before `--locked` broker builds, recreate
    the broker with its exact env (docker inspect first!) **plus
    `-p 127.0.0.1:9310:9310`** (forgetting the port publish breaks host-side
    health/tunnel), reconnect `dokploy-network` + `buzz-sandboxes`. Rebuilding
    an image never touches running sandboxes — users must stop/start.

## Not done / decisions parked

- **Commit.** The changeset is large, verified, and uncommitted — the
  immediate next task. It shares the tree with earlier uncommitted work
  (workflow/CLI/e2e files), so sort by path, sign off every commit (`-s`).
- **Same-spot window stacking**: launched windows of one app open at the same
  pinned position (Alt+Tab mitigates). Cascade would be nicer.
- **ttyd client font in the app's webview** relies on the fallback chain
  (Menlo on macOS) — container fonts don't reach the user's browser.
- **Chrome branding**: parked (see fact 3); dock uses the official Chromium
  logo (BSD; provenance in `desktop/src/features/agents/sandbox/assets/LICENSE-icons.txt`).
  GPL Papirus icons were explicitly rejected — repo is Apache-2.0.
- **Plan phases 2–5**: persistence (disk/session split), activity events,
  Teach a task, skills/routines — see the plan artifact §06.
- Plan artifact decisions D1–D5 (disk retention, transcription, browser-first
  teaching, recording privacy, terminal-vs-M1 rule) — still unanswered.

## Verification habits that worked here

Every claim above was proven against the running system: broker routes with
signed requests over the public URL (`relay.staging.emorise.com/sandbox-viewer`,
NIP-98, port 9310 tunnel signs differently — beware), desktops by screenshot
from inside the container (imagemagick where apt works; pip `python-xlib` +
`pillow` venv where the hardened container blocks apt), dragging via xdotool
with before/after geometry, and the user-facing pages via Playwright against
the real URLs. Throwaway sandboxes were always destroyed (`DELETE`, then
`docker ps` check) — Matterhorn runs production sites.
