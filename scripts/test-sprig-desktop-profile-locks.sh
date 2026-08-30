#!/bin/bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/sprig-desktop-profile-locks.sh
source "$SCRIPT_DIR/sprig-desktop-profile-locks.sh"

TEST_ROOT=$(mktemp -d)
trap 'rm -rf -- "$TEST_ROOT"' EXIT

BROWSER_PROCESS=""
PGREP_ERROR=0
declare -a PGREP_NAMES=()
pgrep() {
    local process_name="${2:-}"
    PGREP_NAMES+=("$process_name")
    [ "$PGREP_ERROR" -eq 0 ] || return 2
    [ "$process_name" = "$BROWSER_PROCESS" ]
}

fail() {
    echo "not ok - $*" >&2
    exit 1
}

assert_exists() {
    [ -e "$1" ] || [ -L "$1" ] || fail "expected $1 to exist"
}

assert_missing() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        fail "expected $1 to be absent"
    fi
}

assert_content() {
    local expected=$1
    local path=$2
    [ "$(cat -- "$path")" = "$expected" ] || fail "unexpected content in $path"
}

assert_contains() {
    local expected=$1
    local path=$2
    grep -Fq -- "$expected" "$path" || fail "expected $path to contain: $expected"
}

profile="$TEST_ROOT/stale-profile"
outside_target="$TEST_ROOT/socket-target"
mkdir -p "$profile/Default/Sessions"
printf 'cookie data\n' >"$profile/Default/Cookies"
printf 'session data\n' >"$profile/Default/Sessions/Tabs_1"
printf 'outside target\n' >"$outside_target"
printf 'old-host-123\n' >"$profile/SingletonLock"
ln -s "$outside_target" "$profile/SingletonSocket"
ln -s "$TEST_ROOT/missing-cookie-target" "$profile/SingletonCookie"

buzz_recover_chromium_profile_locks "$profile"
for artifact in SingletonLock SingletonSocket SingletonCookie; do
    assert_missing "$profile/$artifact"
done
assert_exists "$profile/Default/Cookies"
assert_exists "$profile/Default/Sessions/Tabs_1"
assert_exists "$outside_target"
assert_content "cookie data" "$profile/Default/Cookies"
assert_content "session data" "$profile/Default/Sessions/Tabs_1"
assert_content "outside target" "$outside_target"
echo "ok - stale singleton entries are removed without following symlinks or touching profile data"

buzz_recover_chromium_profile_locks "$profile"
echo "ok - missing singleton entries and repeated recovery are harmless"

live_profile="$TEST_ROOT/live-profile"
mkdir -p "$live_profile"
for artifact in SingletonLock SingletonSocket SingletonCookie; do
    printf 'live\n' >"$live_profile/$artifact"
done
BROWSER_PROCESS="chrome"
if buzz_recover_chromium_profile_locks "$live_profile"; then
    fail "live browser should prevent profile-lock recovery"
else
    status=$?
    [ "$status" -eq 1 ] || fail "expected live-browser status 1, got $status"
fi
BROWSER_PROCESS=""
for artifact in SingletonLock SingletonSocket SingletonCookie; do
    assert_exists "$live_profile/$artifact"
done
echo "ok - a live browser prevents cleanup"

chromium_profile="$TEST_ROOT/chromium-profile"
mkdir -p "$chromium_profile"
printf 'live\n' >"$chromium_profile/SingletonLock"
BROWSER_PROCESS="chromium"
if buzz_recover_chromium_profile_locks "$chromium_profile"; then
    fail "live Chromium should prevent profile-lock recovery"
fi
BROWSER_PROCESS=""
assert_exists "$chromium_profile/SingletonLock"
[[ " ${PGREP_NAMES[*]} " == *" chrome "* ]] || fail "chrome process name was not checked"
[[ " ${PGREP_NAMES[*]} " == *" chromium "* ]] || fail "chromium process name was not checked"
echo "ok - exact Chrome and Chromium process names both prevent cleanup"

error_profile="$TEST_ROOT/process-error-profile"
mkdir -p "$error_profile"
printf 'keep\n' >"$error_profile/SingletonLock"
PGREP_ERROR=1
if buzz_recover_chromium_profile_locks "$error_profile"; then
    fail "process-inspection failure should prevent profile-lock recovery"
else
    status=$?
    [ "$status" -eq 2 ] || fail "expected process-inspection status 2, got $status"
fi
PGREP_ERROR=0
assert_exists "$error_profile/SingletonLock"
echo "ok - process-inspection errors fail closed"

directory_profile="$TEST_ROOT/directory-profile"
mkdir -p "$directory_profile/SingletonLock"
printf 'keep\n' >"$directory_profile/SingletonLock/marker"
printf 'removable\n' >"$directory_profile/SingletonCookie"
if buzz_recover_chromium_profile_locks "$directory_profile"; then
    fail "a directory artifact should report a cleanup failure"
else
    status=$?
    [ "$status" -eq 2 ] || fail "expected cleanup-failure status 2, got $status"
fi
assert_exists "$directory_profile/SingletonLock/marker"
assert_missing "$directory_profile/SingletonCookie"
echo "ok - an unexpected directory is not traversed and failures stay narrowly scoped"

for unsafe_profile in "" "/" "relative/profile"; do
    if buzz_recover_chromium_profile_locks "$unsafe_profile"; then
        fail "unsafe profile path should be rejected: $unsafe_profile"
    fi
done
echo "ok - empty, root, and relative profile paths are rejected"

assert_contains 'source "$BROWSER_DIR/sprig-desktop-profile-locks.sh"' "$SCRIPT_DIR/sprig-desktop-browser.sh"
assert_contains 'buzz-browser \' "$SCRIPT_DIR/sprig-desktop-supervise.sh"
assert_contains 'Exec=buzz-browser %U' "$SCRIPT_DIR/sprig-desktop-browser.desktop"
assert_contains 'launcher_item_app = /home/agent/.local/share/applications/sprig-browser.desktop' "$SCRIPT_DIR/sprig-desktop-tint2rc"
assert_contains 'sprig-desktop-profile-locks.sh /usr/local/bin/sprig-desktop-profile-locks.sh' "$SCRIPT_DIR/../Dockerfile.sprig-desktop"
echo "ok - supervisor, broker wrapper, image, and desktop launcher share the recovery boundary"
