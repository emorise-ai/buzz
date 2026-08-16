#!/bin/bash
# Bring up the agent's desktop and keep it up.
#
# Runs in the background, started by sprig-desktop-entrypoint before it execs
# the harness. Everything here is best-effort by design: a broken desktop must
# NOT stop the agent from working. An agent with no screen can still read,
# write, and run commands; killing the harness because a window manager died
# would turn a cosmetic failure into an outage.
set -uo pipefail

log() { echo "[desktop] $*" >&2; }

: "${DISPLAY:=:1}"
: "${SCREEN_GEOMETRY:=1920x1080x24}"
: "${VNC_PORT:=5901}"
: "${DESKTOP_PORT:=6080}"
: "${CDP_PORT:=9222}"
# Homepage the browser opens on. Set BUZZ_DESKTOP_HOME to change it.
: "${BUZZ_DESKTOP_HOME:=about:blank}"

DISPLAY_NUM="${DISPLAY#:}"
DISPLAY_NUM="${DISPLAY_NUM%%.*}"

# --- X server -----------------------------------------------------------
# -nolisten tcp: the display is reachable only through VNC, never raw X over
# the network.
Xvfb "$DISPLAY" -screen 0 "$SCREEN_GEOMETRY" -nolisten tcp -noreset >/tmp/xvfb.log 2>&1 &
for _ in $(seq 1 50); do
    xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
    sleep 0.2
done
if ! xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
    log "X server did not start; the agent will run without a screen"
    exit 0
fi
log "X server ready on $DISPLAY ($SCREEN_GEOMETRY)"

# --- window manager -----------------------------------------------------
# Without one, Chromium has no decorations, no focus handling, and dialogs
# stack unusably — the human takeover would be miserable.
openbox >/tmp/openbox.log 2>&1 &

# --- VNC export ---------------------------------------------------------
# -shared so the agent's own view and a human's view coexist; -forever so the
# server survives a viewer disconnecting and reconnecting.
#
# No VNC password: the port is never published to the internet. It is reached
# through the broker's authenticated tunnel, exactly like the CDP port. Adding
# a password here would put a second credential in the deploy payload for no
# additional boundary.
x11vnc -display "$DISPLAY" -rfbport "$VNC_PORT" -shared -forever -nopw \
       -noxdamage -ncache 0 -quiet >/tmp/x11vnc.log 2>&1 &

# --- browser-based client ----------------------------------------------
# websockify serves noVNC's static files and bridges the websocket to RFB, so
# a plain browser tab becomes a full mouse+keyboard client.
NOVNC_ROOT=/usr/share/novnc
if [ -d "$NOVNC_ROOT" ]; then
    websockify --web "$NOVNC_ROOT" "$DESKTOP_PORT" "localhost:${VNC_PORT}" \
        >/tmp/websockify.log 2>&1 &
    log "desktop client on :${DESKTOP_PORT} (noVNC)"
else
    log "novnc assets missing; VNC still available on :${VNC_PORT}"
fi

# --- the browser --------------------------------------------------------
# One profile, two controllers. --remote-debugging-port is what lets the agent
# drive; the same window is what the human sees and can grab.
#
# --no-sandbox: the container already drops ALL capabilities and sets
# no-new-privileges, so Chromium's own sandbox has nothing left to drop and
# fails to initialise without it. The container IS the sandbox here.
chromium \
    --remote-debugging-port="${CDP_PORT}" \
    --remote-allow-origins='*' \
    --user-data-dir=/home/agent/.config/chromium \
    --no-first-run \
    --no-default-browser-check \
    --disable-features=Translate \
    --password-store=basic \
    --start-maximized \
    --no-sandbox \
    "$BUZZ_DESKTOP_HOME" \
    >/tmp/chromium.log 2>&1 &

for _ in $(seq 1 60); do
    if curl -sS --max-time 1 "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then
        log "chromium ready; agent control on :${CDP_PORT}"
        break
    fi
    sleep 0.5
done

# --- keep it alive ------------------------------------------------------
# A crashed browser is the one failure worth repairing automatically: the agent
# may be mid-task, and a human may be about to take over. Everything else is
# left alone — restarting X under a live session would be worse than the fault.
while true; do
    sleep 15
    if ! pgrep -x chromium >/dev/null 2>&1; then
        log "chromium exited; restarting"
        chromium \
            --remote-debugging-port="${CDP_PORT}" \
            --remote-allow-origins='*' \
            --user-data-dir=/home/agent/.config/chromium \
            --no-first-run \
            --no-default-browser-check \
            --password-store=basic \
            --no-sandbox \
            "$BUZZ_DESKTOP_HOME" \
            >>/tmp/chromium.log 2>&1 &
    fi
done
