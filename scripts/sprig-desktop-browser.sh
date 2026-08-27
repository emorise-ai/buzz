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
if command -v chromium >/dev/null 2>&1; then
    exec chromium "$@"
fi
exec google-chrome-stable "$@"
