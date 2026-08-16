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

if [ "${BUZZ_DESKTOP_ENABLED:-1}" = "1" ]; then
    /usr/local/bin/sprig-desktop-supervise &
else
    echo "[desktop] disabled by BUZZ_DESKTOP_ENABLED=0" >&2
fi

# exec, not call: the harness takes over PID 1.
exec /usr/local/bin/sprig-entrypoint "$@"
