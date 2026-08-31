#!/bin/bash
# Recover a persistent Chromium profile after its previous container exited.
#
# Chromium's Singleton* entries identify the process and host that currently
# owns a profile. The container is disposable but the profile is not, so those
# entries can outlive the hostname that created them. Keep this helper focused:
# it removes only Chromium's three documented singleton entries and only when
# no local Chrome or Chromium process is running.

buzz_browser_process_running() {
    local process_rows
    local process_state
    local process_name

    if ! process_rows=$(ps -eo stat=,comm= 2>/dev/null); then
        echo "[desktop] could not inspect browser processes; refusing profile-lock recovery" >&2
        return 2
    fi
    while read -r process_state process_name; do
        case "$process_name" in
            chrome | chromium)
                # A zombie has exited and cannot own or mutate the persistent
                # profile; PID 1 may take a moment to reap it in a container.
                [[ "$process_state" == Z* ]] || return 0
                ;;
        esac
    done <<<"$process_rows"
    return 1
}

buzz_recover_chromium_profile_locks() {
    local profile_dir=$1
    local artifact
    local browser_status=0
    local cleanup_failed=0
    local removed=0

    case "$profile_dir" in
        /*) ;;
        *)
            echo "[desktop] Chromium profile path is not absolute; refusing profile-lock recovery" >&2
            return 2
            ;;
    esac
    if [ "$profile_dir" = "/" ]; then
        echo "[desktop] Chromium profile path is unsafe; refusing profile-lock recovery" >&2
        return 2
    fi
    if [ ! -e "$profile_dir" ] && [ ! -L "$profile_dir" ]; then
        return 0
    fi
    if [ ! -d "$profile_dir" ] || [ ! -x "$profile_dir" ] || [ ! -w "$profile_dir" ]; then
        echo "[desktop] Chromium profile directory is inaccessible; refusing profile-lock recovery" >&2
        return 2
    fi

    for artifact in SingletonLock SingletonSocket SingletonCookie; do
        local artifact_path="${profile_dir}/${artifact}"
        # -e excludes dangling symlinks, so test -L separately. rm without -r
        # removes a symlink itself but refuses a directory, keeping the cleanup
        # boundary to one exact directory entry.
        if [ -e "$artifact_path" ] || [ -L "$artifact_path" ]; then
            browser_status=0
            buzz_browser_process_running || browser_status=$?
            if [ "$browser_status" -eq 0 ]; then
                echo "[desktop] browser process started; stopping profile-lock recovery" >&2
                return 1
            fi
            if [ "$browser_status" -ne 1 ]; then
                return 2
            fi
            if rm -- "$artifact_path" 2>/dev/null; then
                removed=1
            else
                echo "[desktop] failed to remove stale Chromium profile lock: ${artifact}" >&2
                cleanup_failed=1
            fi
        fi
    done

    if [ "$removed" -eq 1 ] && [ "$cleanup_failed" -eq 0 ]; then
        echo "[desktop] removed stale Chromium profile locks" >&2
    elif [ "$removed" -eq 1 ]; then
        echo "[desktop] removed some stale Chromium profile locks; recovery remains incomplete" >&2
    fi

    if [ "$cleanup_failed" -eq 1 ]; then
        return 2
    fi
    return 0
}
