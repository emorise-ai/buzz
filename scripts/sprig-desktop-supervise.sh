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

shutdown_desktop() {
    log "shutdown requested; asking Chromium to flush its persistent profile"
    pkill -TERM -x chrome >/dev/null 2>&1 || true
    pkill -TERM -x chromium >/dev/null 2>&1 || true
    for _ in $(seq 1 50); do
        if ! pgrep -x chrome >/dev/null 2>&1 && ! pgrep -x chromium >/dev/null 2>&1; then
            break
        fi
        sleep 0.2
    done
    exit 0
}
trap shutdown_desktop TERM INT

: "${DISPLAY:=:1}"
: "${SCREEN_GEOMETRY:=1920x1080x24}"
: "${VNC_PORT:=5901}"
: "${DESKTOP_PORT:=6080}"
: "${CDP_PORT:=9222}"
: "${TERMINAL_PORT:=7681}"
# Homepage the browser opens on. Set BUZZ_DESKTOP_HOME to change it. Defaults
# to a local dark start page rather than about:blank — about:blank renders
# white even under --force-dark-mode (it's a Chromium-internal page, not a
# web page dark mode affects), and a white flash on launch is exactly the
# "generic Linux box" look the rest of this image avoids.
: "${BUZZ_DESKTOP_HOME:=file:///home/agent/.buzz-start.html}"

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

# --- backdrop ------------------------------------------------------------
# A real wallpaper instead of the X default grey-crosshatch — a flat solid
# color still read as an empty desktop. Falls back to a solid dark fill if
# the wallpaper file or setter is missing, and to the X default only if both
# are gone. Every step is best-effort: a broken backdrop must never block
# the rest of the desktop from coming up.
WALLPAPER=/home/agent/.buzz-wallpaper.png
wallpaper_set=0
if command -v xwallpaper >/dev/null 2>&1 && [ -f "$WALLPAPER" ]; then
    # xwallpaper has no --display flag; it reads $DISPLAY from the
    # environment, which is already exported at the top of this script.
    if DISPLAY="$DISPLAY" xwallpaper --zoom "$WALLPAPER"; then
        log "wallpaper set"
        wallpaper_set=1
    else
        log "xwallpaper failed; falling back to solid backdrop"
    fi
else
    log "xwallpaper or wallpaper file missing; falling back to solid backdrop"
fi
if [ "$wallpaper_set" -eq 0 ]; then
    if command -v xsetroot >/dev/null 2>&1; then
        xsetroot -display "$DISPLAY" -solid '#101215' \
            && log "solid backdrop set" \
            || log "xsetroot failed; default X background will show"
    else
        log "xsetroot not installed; default X background will show"
    fi
fi

# --- window manager -----------------------------------------------------
# Without one, Chromium has no decorations, no focus handling, and dialogs
# stack unusably — the human takeover would be miserable. rc.xml (installed
# to ~/.config/openbox/rc.xml) strips desktop switching (one desktop, so
# there's nothing to switch to) so the screen doesn't read as a Linux
# desktop, and points at the BuzzDark theme (flat, dark — see
# scripts/sprig-desktop-theme/) instead of openbox's stock beveled look.
#
# New windows can also be launched via the root-window right-click menu
# (also in rc.xml) — 4 entries, styled by the same theme — but the primary
# launcher is the tint2 dock started below.
openbox >/tmp/openbox.log 2>&1 &

# --- dock -----------------------------------------------------------------
# tint2: a native, in-screen dock — bottom-centered launcher (the 3 apps'
# real .desktop files, so their real icons show) plus a taskbar reading the
# window manager's own EWMH window list directly (no broker round trip, no
# polling from the app). Started after openbox so its EWMH/_NET_* properties
# already exist for tint2 to read; before picom so the compositor's
# window-type exclusion rule (see sprig-desktop-picom.conf) has a real dock
# window to match against by the time it starts watching for damage events.
# Config (~/.config/tint2/tint2rc, from sprig-desktop-tint2rc) reserves
# screen space for the panel (strut_policy = follow_size) so maximize/
# fullscreen windows stop above the dock instead of running underneath it —
# see that file's header for the math against Chromium/Thunar/terminal's
# hand-placed positions below.
if command -v tint2 >/dev/null 2>&1; then
    tint2 >/tmp/tint2.log 2>&1 &
    log "dock started (tint2)"
else
    log "tint2 not installed; no dock — use the root-window right-click menu"
fi

# --- compositor -----------------------------------------------------------
# picom adds soft drop shadows on windows and a gentle fade-in on open — the
# one piece of visual depth a flat theme + flat WM can't produce on its own.
# --backend xrender (not glx): this is a headless Xvfb display, so there is
# no GPU/DRI to back a GL context: xrender is the software-rendered backend
# that works here. Shadows are on real windows only, never the desktop
# background — a shadow under the wallpaper itself would look like a bug.
if command -v picom >/dev/null 2>&1; then
    picom --backend xrender --config /home/agent/.config/picom.conf \
        >/tmp/picom.log 2>&1 &
    log "compositor started"
else
    log "picom not installed; no window shadows/fades"
fi

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

# --- web terminal ---------------------------------------------------------
# ttyd serves a real bash shell over a websocket (xterm.js), so a human taking
# over gets a terminal alongside the browser without a separate SSH hop.
# -W makes it writable (default is read-only); it runs as the agent user
# since that's who this script itself runs as.
#
# -t fontFamily / -t theme configure xterm.js on the CLIENT side (ttyd embeds
# these as JSON in the page it serves). This renders in the human's own
# browser, not inside this container — a container-installed font (e.g.
# JetBrains Mono, installed below for the GUI terminal and openbox) is
# invisible to it. So the font list is a fallback CHAIN: "JetBrains Mono"
# first for anyone who has it installed locally, then SFMono-Regular and
# Menlo (both ship with macOS, covering the primary target — a human on a
# Mac using the desktop-viewer webview), then a generic monospace as the
# last resort everywhere else.
TERMINAL_CWD=/workspace
[ -d "$TERMINAL_CWD" ] || TERMINAL_CWD=/home/agent

if command -v ttyd >/dev/null 2>&1; then
    ttyd --port "$TERMINAL_PORT" --interface 0.0.0.0 --writable \
        --cwd "$TERMINAL_CWD" \
        -t 'fontFamily=JetBrains Mono, SFMono-Regular, Menlo, monospace' \
        -t 'fontSize=14' \
        -t 'theme={"background":"#101215","foreground":"#d8dee9","cursor":"#6fd6a8"}' \
        bash >/tmp/ttyd.log 2>&1 &
    log "web terminal on :${TERMINAL_PORT} (cwd $TERMINAL_CWD)"
else
    log "ttyd not installed; no web terminal"
fi

# --- the browser --------------------------------------------------------
# One profile, two controllers. --remote-debugging-port is what lets the agent
# drive; the same window is what the human sees and can grab.
#
# --no-sandbox: the container already drops ALL capabilities and sets
# no-new-privileges, so Chromium's own sandbox has nothing left to drop and
# fails to initialise without it. The container IS the sandbox here.
#
# --test-type: suppresses the "unsupported command-line flag" warning bar that
# --no-sandbox otherwise pins to every window. The warning is aimed at people
# who disabled the sandbox on a real desktop; here it only alarms the human
# watching the agent's screen about a property the container already provides.
#
# --force-dark-mode / --enable-features=WebUIDarkMode: the browser chrome
# (toolbar, menus, internal pages) renders dark instead of Chromium's default
# light theme. Neither flag is --kiosk — the tab strip, URL bar, and window
# controls stay, because a human taking over needs them.
#
# --window-size / --window-position: Chromium opens as a large centered
# window, not fullscreen. An earlier version of this image forced it
# undecorated+maximized via an openbox rule — reverted, because on a
# 1920x1080 screen that meant Chromium WAS the whole screen: the wallpaper
# was invisible, there was no sense of a desktop underneath, and the
# Computer view just looked like "a browser", not a computer. A window with
# visible wallpaper margin, a normal title bar, and room to see Thunar or
# the terminal alongside it reads as a real desktop instead. 1700x950 with
# generous margin on a 1920x1080 screen: ((1920-1700)/2, (1080-950)/2).
# buzz-browser resolves to Google Chrome on amd64 and Chromium elsewhere;
# both accept this flag set, and both get the same profile dir so a switch
# of binary never orphans the human's logins.
#
# --use-gl=angle --use-angle=swiftshader-webgl / --enable-unsafe-swiftshader /
# --ignore-gpu-blocklist: there is no GPU or DRI device in this container (see
# the picom --backend xrender note above), so Chromium's default GPU checks
# blocklist this environment and WebGL reports "not enabled" to pages (e.g.
# Onshape, Figma, any three.js/WebGL app) even though the CPU has plenty of
# headroom to software-render it via SwiftShader. These flags force Chromium
# onto its CPU-rasterized WebGL path instead of trying (and failing) to find
# real GPU hardware.
CHROMIUM_PROFILE=/home/agent/.config/chromium

launch_browser() {
    local session_args=()
    if [ -f "$CHROMIUM_PROFILE/Local State" ]; then
        # Do not append the Buzz start page here: an explicit URL would create
        # an extra tab alongside Chromium's restored persistent session.
        session_args+=(--restore-last-session)
        log "restoring persistent browser session"
    else
        session_args+=("$BUZZ_DESKTOP_HOME")
        log "starting fresh browser profile"
    fi

    buzz-browser \
        --remote-debugging-port="${CDP_PORT}" \
        --remote-allow-origins='*' \
        --user-data-dir="$CHROMIUM_PROFILE" \
        --no-first-run \
        --no-default-browser-check \
        --disable-features=Translate \
        --password-store=basic \
        --force-dark-mode \
        --enable-features=WebUIDarkMode \
        --window-size=1700,950 \
        --window-position=110,65 \
        --no-sandbox \
        --test-type \
        --use-gl=angle \
        --use-angle=swiftshader-webgl \
        --enable-unsafe-swiftshader \
        --ignore-gpu-blocklist \
        "${session_args[@]}" \
        >>/tmp/chromium.log 2>&1 &
}

: >/tmp/chromium.log
launch_browser

for _ in $(seq 1 60); do
    if curl -sS --max-time 1 "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then
        log "browser ready; agent control on :${CDP_PORT}"
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
    # Chrome's process name is "chrome", Chromium's is "chromium" — check both.
    if ! pgrep -x chrome >/dev/null 2>&1 && ! pgrep -x chromium >/dev/null 2>&1; then
        log "browser exited; restarting"
        launch_browser
    fi
    if command -v ttyd >/dev/null 2>&1 && ! pgrep -x ttyd >/dev/null 2>&1; then
        log "ttyd exited; restarting"
        ttyd --port "$TERMINAL_PORT" --interface 0.0.0.0 --writable \
            --cwd "$TERMINAL_CWD" \
            -t 'fontFamily=JetBrains Mono, SFMono-Regular, Menlo, monospace' \
            -t 'fontSize=14' \
            -t 'theme={"background":"#101215","foreground":"#d8dee9","cursor":"#6fd6a8"}' \
            bash >>/tmp/ttyd.log 2>&1 &
    fi
done
