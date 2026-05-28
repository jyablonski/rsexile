#!/usr/bin/env bash
set -u
# Keep cleanup explicit in this launcher. Avoid set -e so failed shutdown probes
# or child process exits do not skip the cleanup path.

# Needs `wait -n -p` (bash 5.1+) for the child-wait loop below.
if (( BASH_VERSINFO[0] < 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] < 1) )); then
    echo "rsexile launcher requires bash >= 5.1 (found ${BASH_VERSION})" >&2
    exit 70
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
LOG_DIR="${XDG_STATE_HOME:-${HOME}/.local/state}/rsexile"
RSEXILE_OUTPUT_LOG="${LOG_DIR}/rsexile.log"
RSEXILE_PREV_LOG="${LOG_DIR}/rsexile.log.prev"

RSEXILE_PID=""
GAME_PID=""
GAME_STATUS=0

cleanup() {
    if [[ -n "${RSEXILE_PID}" ]] && kill -0 "${RSEXILE_PID}" 2>/dev/null; then
        kill "${RSEXILE_PID}" 2>/dev/null || true
        wait "${RSEXILE_PID}" 2>/dev/null || true
    fi
}

terminate() {
    local signal="${1:-TERM}"
    local status=143

    trap - EXIT

    if [[ -n "${GAME_PID}" ]] && kill -0 "${GAME_PID}" 2>/dev/null; then
        kill -TERM "${GAME_PID}" 2>/dev/null || true
        wait "${GAME_PID}" 2>/dev/null || true
    fi

    cleanup

    case "${signal}" in
        INT) status=130 ;;
        HUP) status=129 ;;
        TERM) status=143 ;;
    esac

    exit "${status}"
}

trap cleanup EXIT
trap 'terminate INT' INT
trap 'terminate TERM' TERM
trap 'terminate HUP' HUP

if [[ "$#" -eq 0 ]]; then
    echo "Usage: $0 %command%" >&2
    exit 64
fi

mkdir -p "${LOG_DIR}"

# Kill any stale overlay processes from a previous launch. Use -x to match
# the process name exactly (not a substring of the command line), so we
# do not accidentally kill unrelated processes that happen to mention
# "rsexile" in their args. Scope to the current user for extra safety.
pkill -u "$(id -u)" -x rsexile 2>/dev/null || true

# Prefer the installed binary, then the in-tree release build, then debug.
if [[ -x "${HOME}/.local/bin/rsexile" ]]; then
    rsexile_cmd=("${HOME}/.local/bin/rsexile")
elif [[ -x "${PROJECT_DIR}/target/release/rsexile" ]]; then
    rsexile_cmd=("${PROJECT_DIR}/target/release/rsexile")
elif [[ -x "${PROJECT_DIR}/target/debug/rsexile" ]]; then
    rsexile_cmd=("${PROJECT_DIR}/target/debug/rsexile")
else
    echo "rsexile binary not found. Build it with: cargo build --release" >&2
    exit 69
fi

if [[ -n "${RSEXILE_LOG:-}" ]]; then
    rsexile_cmd+=(--log "${RSEXILE_LOG}")
fi

# Rotate the previous session's log so we keep one prior run for debugging
# instead of clobbering it on every launch.
if [[ -f "${RSEXILE_OUTPUT_LOG}" ]]; then
    mv -f "${RSEXILE_OUTPUT_LOG}" "${RSEXILE_PREV_LOG}" 2>/dev/null || true
fi

"${rsexile_cmd[@]}" >"${RSEXILE_OUTPUT_LOG}" 2>&1 &
RSEXILE_PID=$!

"$@" &
GAME_PID=$!

while [[ -n "${GAME_PID}" ]]; do
    EXITED_PID=""
    wait_pids=("${GAME_PID}")
    if [[ -n "${RSEXILE_PID}" ]]; then
        wait_pids+=("${RSEXILE_PID}")
    fi

    wait -n -p EXITED_PID "${wait_pids[@]}" 2>/dev/null
    CHILD_STATUS=$?

    if [[ "${EXITED_PID}" == "${GAME_PID}" ]]; then
        GAME_STATUS="${CHILD_STATUS}"
        GAME_PID=""
    elif [[ -n "${RSEXILE_PID}" && "${EXITED_PID}" == "${RSEXILE_PID}" ]]; then
        printf '%s rsexile exited early with status %s; see %s\n' \
            "$(date -Is)" "${CHILD_STATUS}" "${RSEXILE_OUTPUT_LOG}" >&2
        RSEXILE_PID=""
    else
        # Defensive: wait returned without identifying a known child
        # (e.g. wait failed with no children remaining). Bail out so we
        # do not spin forever.
        break
    fi
done

exit "${GAME_STATUS}"
