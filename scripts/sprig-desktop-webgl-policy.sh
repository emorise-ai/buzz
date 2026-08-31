#!/bin/bash
# Pure runtime policy for translating health-command exit statuses into the
# supervisor's next action and debounce count. Keeping this free of process
# operations makes the destructive restart boundary executable in tests.

buzz_runtime_webgl_policy() {
    local health_status=$1
    local failures=$2
    local threshold=$3

    case "$health_status" in
        0)
            printf 'healthy 0\n'
            ;;
        1)
            failures=$((failures + 1))
            if [ "$failures" -ge "$threshold" ]; then
                printf 'restart 0\n'
            else
                printf 'failure %s\n' "$failures"
            fi
            ;;
        2)
            printf 'inconclusive 0\n'
            ;;
        *)
            printf 'unavailable 0\n'
            ;;
    esac
}
