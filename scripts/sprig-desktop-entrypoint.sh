#!/bin/bash
# Entrypoint for the desktop-enabled agent image.
#
# Starts the desktop in the background, then hands off to the base image's
# entrypoint, which execs buzz-acp as PID 1. That ordering is deliberate and
# load-bearing: the harness must remain the process that receives the
# container's termination signal (docs/remote-agents.md, L1 launcher
# obligations — "a wrapper that swallows the signal conforms to nothing").
#
# The desktop is explicitly optional. If it fails to come up, the supervisor
# logs and exits; the agent still runs with its full non-visual toolchain.
set -euo pipefail

# `/home/agent` is persistent, so image updates cannot rely on files baked
# directly into that directory. Refresh only the Buzz-owned paths from an
# image-owned source tree; `cp` merges directories and leaves every unrelated
# browser, download, workspace, and user-created file untouched.
DESKTOP_DEFAULTS=/usr/local/share/buzz-desktop/home
if [ -d "$DESKTOP_DEFAULTS" ]; then
    while IFS= read -r -d '' source; do
        relative=${source#"$DESKTOP_DEFAULTS"/}
        destination=/home/agent/$relative
        parent=/home/agent
        safe=1
        IFS='/' read -r -a parts <<< "$(dirname "$relative")"
        for part in "${parts[@]}"; do
            [ "$part" = "." ] && continue
            parent=$parent/$part
            if [ -L "$parent" ]; then
                echo "[desktop] skipped managed default $relative: parent is a symlink" >&2
                safe=0
                break
            fi
            mkdir -p "$parent"
        done
        [ "$safe" = "1" ] || continue
        # Replace a symlink at the exact managed path instead of following it
        # into an unrelated browser or user file.
        [ ! -L "$destination" ] || rm -- "$destination"
        if ! cp -- "$source" "$destination"; then
            echo "[desktop] could not refresh managed default $relative" >&2
        fi
    done < <(find "$DESKTOP_DEFAULTS" -type f -print0)
fi

if [ "${BUZZ_DESKTOP_ENABLED:-1}" = "1" ]; then
    /usr/local/bin/sprig-desktop-supervise &
else
    echo "[desktop] disabled by BUZZ_DESKTOP_ENABLED=0" >&2
fi

# exec, not call: the harness takes over PID 1.
exec /usr/local/bin/sprig-entrypoint "$@"
