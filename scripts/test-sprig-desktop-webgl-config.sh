#!/bin/bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
BROWSER_WRAPPER="$SCRIPT_DIR/sprig-desktop-browser.sh"
SUPERVISOR="$SCRIPT_DIR/sprig-desktop-supervise.sh"
HEALTH_CHECK="$SCRIPT_DIR/sprig-desktop-webgl-health.mjs"
HEALTH_POLICY="$SCRIPT_DIR/sprig-desktop-webgl-policy.sh"
DOCKERFILE="$REPO_ROOT/Dockerfile.sprig-desktop"
JUSTFILE="$REPO_ROOT/Justfile"

fail() {
    echo "not ok - $*" >&2
    exit 1
}

assert_contains() {
    local expected=$1
    local path=$2
    grep -Fq -- "$expected" "$path" || fail "expected $path to contain: $expected"
}

assert_absent() {
    local unexpected=$1
    shift
    if grep -Fq -- "$unexpected" "$@"; then
        fail "unexpected renderer setting remains: $unexpected"
    fi
}

assert_contains 'libegl-mesa0 \' "$DOCKERFILE"
assert_contains 'libgl1-mesa-dri \' "$DOCKERFILE"
assert_contains 'libgles2 \' "$DOCKERFILE"
assert_contains 'sprig-desktop-webgl-health.mjs /usr/local/bin/sprig-desktop-webgl-health.mjs' "$DOCKERFILE"
assert_contains 'sprig-desktop-webgl-policy.sh /usr/local/bin/sprig-desktop-webgl-policy.sh' "$DOCKERFILE"
echo "ok - image installs the Mesa runtime and CDP health check"

assert_contains 'BROWSER_ARGUMENTS+=(' "$BROWSER_WRAPPER"
assert_contains '--use-gl=angle' "$BROWSER_WRAPPER"
assert_contains '--use-angle=gl' "$BROWSER_WRAPPER"
assert_contains '--ignore-gpu-blocklist' "$BROWSER_WRAPPER"
assert_absent '--use-gl=' "$SUPERVISOR" "$SCRIPT_DIR/sprig-desktop-browser.desktop" "$SCRIPT_DIR/sprig-desktop-menu.xml"
assert_absent '--use-angle=' "$SUPERVISOR" "$SCRIPT_DIR/sprig-desktop-browser.desktop" "$SCRIPT_DIR/sprig-desktop-menu.xml"
assert_absent '--use-angle=swiftshader-webgl' "$BROWSER_WRAPPER" "$SUPERVISOR" "$SCRIPT_DIR/sprig-desktop-browser.desktop" "$SCRIPT_DIR/sprig-desktop-menu.xml"
assert_absent '--enable-unsafe-swiftshader' "$SUPERVISOR" "$SCRIPT_DIR/sprig-desktop-browser.desktop" "$SCRIPT_DIR/sprig-desktop-menu.xml"
echo "ok - browser callers delegate one Mesa renderer policy to buzz-browser"

assert_contains 'wait_for_browser_health || startup_health_status=$?' "$SUPERVISOR"
assert_contains 'browser_webgl_healthy || runtime_health_status=$?' "$SUPERVISOR"
assert_contains '0|1|2) return "$health_status"' "$SUPERVISOR"
assert_contains 'restart_unhealthy_browser || true' "$SUPERVISOR"
assert_contains 'WEBGL_RUNTIME_FAILURE_THRESHOLD:=2' "$SUPERVISOR"
assert_contains 'WEBGL_HEALTH_INTERVAL_SECONDS:=15' "$SUPERVISOR"
assert_contains 'buzz_runtime_webgl_policy' "$SUPERVISOR"
assert_contains 'WEBGL_STARTUP_TIMEOUT_SECONDS:=35' "$SUPERVISOR"
assert_contains 'renderer recovery deferred because the health check is unavailable' "$SUPERVISOR"
assert_contains 'renderer recovery deferred because startup health remained inconclusive' "$SUPERVISOR"
assert_contains 'renderer health inconclusive; retrying next interval' "$SUPERVISOR"
assert_contains 'BROWSER_HEALTH_FAILURES=0' "$SUPERVISOR"
assert_contains 'WebGL health unavailable: node is not installed' "$SUPERVISOR"
assert_contains 'stop_browser_gracefully' "$SUPERVISOR"
assert_contains 'browser process inspection failed; recovery deferred' "$SUPERVISOR"
assert_contains 'refusing to start a competing profile owner' "$SUPERVISOR"
assert_contains 'pkill -TERM -x chromium' "$SUPERVISOR"
assert_contains 'node "$WEBGL_HEALTH_SCRIPT"' "$SUPERVISOR"
assert_absent '/json/version' "$SUPERVISOR"
assert_contains 'node --test ./scripts/test-sprig-desktop-webgl-health.mjs' "$JUSTFILE"
assert_contains 'SystemInfo.getInfo' "$HEALTH_CHECK"
assert_contains 'healthExitCode(error)' "$HEALTH_CHECK"
echo "ok - renderer readiness and ongoing recovery are gated by CDP health"

# shellcheck source=scripts/sprig-desktop-webgl-policy.sh
source "$HEALTH_POLICY"
assert_policy() {
    local expected=$1
    shift
    local actual
    actual=$(buzz_runtime_webgl_policy "$@")
    [ "$actual" = "$expected" ] \
        || fail "health policy for status $1 returned '$actual', expected '$expected'"
}
assert_policy 'healthy 0' 0 1 2
assert_policy 'failure 1' 1 0 2
assert_policy 'restart 0' 1 1 2
assert_policy 'inconclusive 0' 2 1 2
assert_policy 'unavailable 0' 3 1 2
assert_policy 'unavailable 0' 137 1 2
echo "ok - executable health policy restarts only confirmed consecutive failures"

TEST_ROOT=$(mktemp -d)
trap 'rm -rf -- "$TEST_ROOT"' EXIT
mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/profile-first" "$TEST_ROOT/profile-effective"

cat >"$TEST_ROOT/bin/ps" <<'EOF'
#!/bin/bash
exit 0
EOF
cat >"$TEST_ROOT/bin/flock" <<'EOF'
#!/bin/bash
exit 0
EOF
cat >"$TEST_ROOT/bin/chromium" <<EOF
#!/bin/bash
printf '%s\n' "\$@" >"$TEST_ROOT/arguments"
EOF
chmod +x "$TEST_ROOT/bin/ps" "$TEST_ROOT/bin/flock" "$TEST_ROOT/bin/chromium"

PATH="$TEST_ROOT/bin:$PATH" bash "$BROWSER_WRAPPER" \
    --new-window \
    --user-data-dir="$TEST_ROOT/profile-first" \
    --user-data-dir="$TEST_ROOT/profile-effective" \
    --use-gl=disabled \
    --use-angle=swiftshader-webgl \
    --enable-unsafe-swiftshader \
    --disable-gpu=true \
    --disable-webgl=1 \
    --disable-webgl2 \
    --disable-3d-apis=true \
    --disable-software-rasterizer=1 \
    --disable-gpu-compositing=true \
    --ignore-gpu-blocklist \
    https://example.invalid/

[ "$(grep -Fxc -- '--use-gl=angle' "$TEST_ROOT/arguments")" -eq 1 ] \
    || fail "Chromium did not receive exactly one ANGLE GL selector"
[ "$(grep -Fxc -- '--use-angle=gl' "$TEST_ROOT/arguments")" -eq 1 ] \
    || fail "Chromium did not receive exactly one Mesa OpenGL selector"
[ "$(grep -Fxc -- '--ignore-gpu-blocklist' "$TEST_ROOT/arguments")" -eq 1 ] \
    || fail "Chromium did not receive exactly one blocklist override"
assert_absent 'swiftshader' "$TEST_ROOT/arguments"
assert_absent '--enable-unsafe-swiftshader' "$TEST_ROOT/arguments"
assert_absent '--disable-gpu' "$TEST_ROOT/arguments"
assert_absent '--disable-webgl' "$TEST_ROOT/arguments"
assert_absent '--disable-webgl2' "$TEST_ROOT/arguments"
assert_absent '--disable-3d-apis' "$TEST_ROOT/arguments"
assert_absent '--disable-software-rasterizer' "$TEST_ROOT/arguments"
assert_absent '--disable-gpu-compositing' "$TEST_ROOT/arguments"
[ "$(grep -c -- '^--user-data-dir=' "$TEST_ROOT/arguments")" -eq 1 ] \
    || fail "Chromium did not receive exactly one effective profile argument"
assert_contains '--user-data-dir='"$TEST_ROOT/profile-effective" "$TEST_ROOT/arguments"
assert_absent "$TEST_ROOT/profile-first" "$TEST_ROOT/arguments"
assert_contains 'https://example.invalid/' "$TEST_ROOT/arguments"
echo "ok - conflicting caller flags are discarded without losing profile or URL arguments"
