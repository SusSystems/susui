#!/usr/bin/env bash
set -euo pipefail

trap 'echo "ERROR: susui-push failed at line $LINENO (exit $?)" >&2' ERR

# Per-instance cache directory keyed on flake ref
INSTANCE_ID=$(echo -n "$SUSUI_FLAKE_REF" | sha256sum | cut -d' ' -f1)
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/susui/$INSTANCE_ID"
HASH_FILE="$CACHE_DIR/last-scan-hash"
mkdir -p "$CACHE_DIR"

# Generate susui.yaml from env vars, or use SUSUI_WORKING_DIR
if [ -n "${SUSUI_DASHBOARD_REPO:-}" ] || [ -n "${SUSUI_STATUS_REPO:-}" ]; then
    CONFIG="$CACHE_DIR/susui.yaml"
    : > "$CONFIG"

    # dashboard_push section
    if [ -n "${SUSUI_DASHBOARD_REPO:-}" ]; then
        {
            echo "dashboard_push:"
            [ -n "${SUSUI_DASHBOARD_OWNER:-}" ]   && echo "  owner: $SUSUI_DASHBOARD_OWNER"
            [ -n "${SUSUI_DASHBOARD_ORG:-}" ]     && echo "  org: $SUSUI_DASHBOARD_ORG"
            echo "  repo: $SUSUI_DASHBOARD_REPO"
            [ -n "${SUSUI_DASHBOARD_BRANCH:-}" ]  && echo "  branch: $SUSUI_DASHBOARD_BRANCH"
            [ -n "${SUSUI_DASHBOARD_HOST:-}" ]    && echo "  host: $SUSUI_DASHBOARD_HOST"
            [ -n "${SUSUI_DASHBOARD_CNAME:-}" ]   && echo "  cname: $SUSUI_DASHBOARD_CNAME"
            [ -n "${SUSUI_DASHBOARD_MESSAGE:-}" ] && echo "  commit_message: \"$SUSUI_DASHBOARD_MESSAGE\""
        } >> "$CONFIG"
    fi

    # status_push section
    if [ -n "${SUSUI_STATUS_REPO:-}" ]; then
        {
            echo "status_push:"
            echo "  - input: ${SUSUI_STATUS_INPUT:-src}"
            echo "    type: ${SUSUI_STATUS_TYPE:-github}"
            [ -n "${SUSUI_STATUS_OWNER:-}" ]      && echo "    owner: $SUSUI_STATUS_OWNER"
            [ -n "${SUSUI_STATUS_ORG:-}" ]        && echo "    org: $SUSUI_STATUS_ORG"
            echo "    repo: $SUSUI_STATUS_REPO"
            [ -n "${SUSUI_STATUS_METHOD:-}" ]     && echo "    method: $SUSUI_STATUS_METHOD"
            [ -n "${SUSUI_STATUS_CONTEXT:-}" ]    && echo "    context: \"$SUSUI_STATUS_CONTEXT\""
            [ -n "${SUSUI_STATUS_CHECK_NAME:-}" ] && echo "    check_name: \"$SUSUI_STATUS_CHECK_NAME\""
            [ -n "${SUSUI_STATUS_TARGET_URL:-}" ] && echo "    target_url: $SUSUI_STATUS_TARGET_URL"
            [ -n "${SUSUI_STATUS_HOST:-}" ]       && echo "    host: $SUSUI_STATUS_HOST"
        } >> "$CONFIG"
    fi

    cd "$CACHE_DIR"
else
    cd "$SUSUI_WORKING_DIR"
fi

# Scan and hash the current build state
# scan may exit non-zero when some derivations fail to build — that's expected
CURRENT_HASH=$("$SUSUI_BIN" scan "$SUSUI_FLAKE_REF" --json || true)
CURRENT_HASH=$(echo "$CURRENT_HASH" | sha256sum | cut -d' ' -f1)

# Compare to cached hash
if [ -f "$HASH_FILE" ] && [ "$(cat "$HASH_FILE")" = "$CURRENT_HASH" ]; then
    echo "Build state unchanged, skipping push."
    exit 0
fi

echo "Build state changed, pushing..."

PUSH_OK=true

# Push commit status
"$SUSUI_BIN" push-status "$SUSUI_FLAKE_REF" || { echo "warning: push-status failed"; PUSH_OK=false; }

# Push dashboard
"$SUSUI_BIN" push-dashboard "$SUSUI_FLAKE_REF" || { echo "warning: push-dashboard failed"; PUSH_OK=false; }

# Only cache the hash when all pushes succeeded, so failed pushes are retried
if $PUSH_OK; then
    echo "$CURRENT_HASH" > "$HASH_FILE"
else
    echo "One or more pushes failed — will retry on next trigger."
    rm -f "$HASH_FILE"
fi
