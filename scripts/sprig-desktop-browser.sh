#!/bin/bash
# buzz-browser — launch whichever browser this image carries.
#
# Chromium is the browser of this image. Google Chrome was tried and
# reverted — its crashpad crash reporter SIGTRAPs under the sandbox's
# cap-drop ALL + no-new-privileges hardening (see Dockerfile.sprig-desktop
# for the full test record), while Chromium ships without that layer and
# is the same engine with the same UI. Everything that starts a browser —
# supervise, the openbox root menu — goes through this one name so a
# future browser swap happens in exactly one place. Chromium is preferred
# even if a Chrome binary is present, so a stale layer can never resurrect
# the crash loop.
set -uo pipefail

BROWSER_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/sprig-desktop-profile-locks.sh
if ! source "$BROWSER_DIR/sprig-desktop-profile-locks.sh"; then
    echo "[desktop] profile-lock recovery helper could not be loaded" >&2
    exit 1
fi

if command -v chromium >/dev/null 2>&1; then
    BROWSER_COMMAND=chromium
else
    BROWSER_COMMAND=google-chrome-stable
fi

CHROMIUM_PROFILE=/home/agent/.config/chromium
declare -a BROWSER_ARGUMENTS=()
for argument in "$@"; do
    case "$argument" in
        # Keep only the final requested profile and append it exactly once.
        # Cleanup and Chromium must agree on the same effective directory.
        --user-data-dir=*)
            CHROMIUM_PROFILE=${argument#--user-data-dir=}
            continue
            ;;
        # Renderer selection belongs to this shared boundary. Discard stale
        # or caller-supplied policy that could replace or disable WebGL before
        # adding the image's known-good Mesa path below.
        --use-gl=* | --use-angle=* | --enable-unsafe-swiftshader | --ignore-gpu-blocklist) continue ;;
        --disable-gpu | --disable-gpu=* | --disable-webgl | --disable-webgl=* | --disable-webgl2 | --disable-webgl2=*) continue ;;
        --disable-3d-apis | --disable-3d-apis=* | --disable-software-rasterizer | --disable-software-rasterizer=* | --disable-gpu-compositing | --disable-gpu-compositing=*) continue ;;
    esac
    BROWSER_ARGUMENTS+=("$argument")
done

# Xvfb exposes no DRI device. ANGLE's OpenGL backend therefore resolves through
# Mesa's llvmpipe software renderer, which keeps a WebGL2 context alive under
# Onshape's workload. Keep these arguments here so supervisor, broker, dock,
# and Openbox launches cannot drift to SwiftShader or disable WebGL.
BROWSER_ARGUMENTS+=(
    --user-data-dir="$CHROMIUM_PROFILE"
    --use-gl=angle
    --use-angle=gl
    --ignore-gpu-blocklist
)

# Every browser entry point uses this wrapper. Serialize the short transition
# from stale-lock cleanup until the new browser process is visible, so a dock,
# broker, or supervisor launch cannot race cleanup against another launch.
exec 9>/tmp/buzz-chromium-profile-launch.lock
flock 9

browser_status=0
buzz_browser_process_running || browser_status=$?
if [ "$browser_status" -eq 0 ]; then
    flock -u 9
    exec "$BROWSER_COMMAND" "${BROWSER_ARGUMENTS[@]}"
fi

recovery_status=0
buzz_recover_chromium_profile_locks "$CHROMIUM_PROFILE" || recovery_status=$?
if [ "$recovery_status" -eq 1 ]; then
    # A browser appeared between the process check and recovery. Do not touch
    # its profile; let Chromium route this request to the live instance.
    flock -u 9
    exec "$BROWSER_COMMAND" "${BROWSER_ARGUMENTS[@]}"
fi
if [ "$recovery_status" -ne 0 ]; then
    echo "[desktop] profile-lock recovery was incomplete; Chromium will validate the profile" >&2
fi

"$BROWSER_COMMAND" "${BROWSER_ARGUMENTS[@]}" &
browser_pid=$!
for _ in $(seq 1 100); do
    if buzz_browser_process_running; then
        break
    fi
    if ! kill -0 "$browser_pid" >/dev/null 2>&1; then
        break
    fi
    sleep 0.01
done
flock -u 9
wait "$browser_pid"
