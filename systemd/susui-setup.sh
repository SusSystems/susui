#!/usr/bin/env bash
set -euo pipefail

# ── Colors & helpers ─────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'

ok()   { printf "  ${GREEN}✓${RESET} %s\n" "$*"; }
warn() { printf "  ${YELLOW}!${RESET} %s\n" "$*"; }
err()  { printf "  ${RED}✗${RESET} %s\n" "$*"; }
step() { printf "\n${BOLD}▸ %s${RESET}\n" "$*"; }

ask() {
    local prompt="$1" default="$2" var="$3"
    if [ -n "$default" ]; then
        read -rp "  $prompt [$default]: " value
        eval "$var=\"\${value:-$default}\""
    else
        read -rp "  $prompt: " value
        eval "$var=\"\$value\""
    fi
}

ask_yn() {
    local prompt="$1" default="$2"
    local suffix
    if [ "$default" = "Y" ]; then suffix="[Y/n]"; else suffix="[y/N]"; fi
    read -rp "  $prompt $suffix: " answer
    answer="${answer:-$default}"
    [[ "$answer" =~ ^[Yy] ]]
}

# ── Locate bundled push script ───────────────────────────────────────
SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
PUSH_SCRIPT=""

if [ -f "$SCRIPT_DIR/../share/susui/susui-push.sh" ]; then
    PUSH_SCRIPT="$SCRIPT_DIR/../share/susui/susui-push.sh"
elif [ -f "$SCRIPT_DIR/susui-push.sh" ]; then
    PUSH_SCRIPT="$SCRIPT_DIR/susui-push.sh"
fi

if [ -z "$PUSH_SCRIPT" ]; then
    err "Could not find susui-push.sh"
    echo "  Looked in:"
    echo "    $SCRIPT_DIR/../share/susui/"
    echo "    $SCRIPT_DIR/"
    exit 1
fi

# ── Paths ────────────────────────────────────────────────────────────
CONFIG_DIR="$HOME/.config/susui"
SYSTEMD_DIR="$HOME/.config/systemd/user"
PUSH_ENV="$CONFIG_DIR/push.env"
TOKEN_ENV="$CONFIG_DIR/github-token.env"
PATH_UNIT="$SYSTEMD_DIR/susui-push.path"
TIMER_UNIT="$SYSTEMD_DIR/susui-push.timer"
SERVICE_UNIT="$SYSTEMD_DIR/susui-push.service"

# ── Detection phase ──────────────────────────────────────────────────
UPGRADE=false
UNITS_EXIST=false
CONFIG_EXIST=false
TOKEN_EXIST=false

[ -f "$PATH_UNIT" ] && UNITS_EXIST=true
[ -f "$PUSH_ENV" ] && CONFIG_EXIST=true
[ -f "$TOKEN_ENV" ] && TOKEN_EXIST=true

# Load existing config as defaults
D_FLAKE_REF="" D_BIN="susui"
D_DASH_ENABLED="" D_DASH_OWNER="" D_DASH_REPO="" D_DASH_BRANCH="gh-pages"
D_DASH_HOST="" D_DASH_CNAME="" D_DASH_MESSAGE=""
D_STATUS_ENABLED="" D_STATUS_INPUT="" D_STATUS_TYPE=""
D_STATUS_OWNER="" D_STATUS_REPO="" D_STATUS_METHOD="commit_status"
D_STATUS_CONTEXT="nix-build/local" D_STATUS_CHECK_NAME="" D_STATUS_TARGET_URL="" D_STATUS_HOST=""

if $CONFIG_EXIST; then
    # shellcheck source=/dev/null
    source "$PUSH_ENV"
    D_FLAKE_REF="${SUSUI_FLAKE_REF:-}"
    D_BIN="${SUSUI_BIN:-susui}"
    D_DASH_OWNER="${SUSUI_DASHBOARD_OWNER:-}"
    D_DASH_REPO="${SUSUI_DASHBOARD_REPO:-}"
    D_DASH_BRANCH="${SUSUI_DASHBOARD_BRANCH:-gh-pages}"
    D_DASH_HOST="${SUSUI_DASHBOARD_HOST:-}"
    D_DASH_CNAME="${SUSUI_DASHBOARD_CNAME:-}"
    D_DASH_MESSAGE="${SUSUI_DASHBOARD_MESSAGE:-}"
    [ -n "$D_DASH_REPO" ] && D_DASH_ENABLED="Y"
    D_STATUS_INPUT="${SUSUI_STATUS_INPUT:-src}"
    D_STATUS_TYPE="${SUSUI_STATUS_TYPE:-github}"
    D_STATUS_OWNER="${SUSUI_STATUS_OWNER:-}"
    D_STATUS_REPO="${SUSUI_STATUS_REPO:-}"
    D_STATUS_METHOD="${SUSUI_STATUS_METHOD:-commit_status}"
    D_STATUS_CONTEXT="${SUSUI_STATUS_CONTEXT:-nix-build/local}"
    D_STATUS_CHECK_NAME="${SUSUI_STATUS_CHECK_NAME:-}"
    D_STATUS_TARGET_URL="${SUSUI_STATUS_TARGET_URL:-}"
    D_STATUS_HOST="${SUSUI_STATUS_HOST:-}"
    [ -n "$D_STATUS_REPO" ] && D_STATUS_ENABLED="Y"
fi

printf "\n${BOLD}susui-setup${RESET} — interactive installer for susui push units\n"

if $UNITS_EXIST || $CONFIG_EXIST || $TOKEN_EXIST; then
    step "Existing installation detected"
    $UNITS_EXIST  && ok "Systemd units installed"  || warn "Systemd units not found"
    $CONFIG_EXIST && ok "Push config exists"        || warn "Push config not found"
    $TOKEN_EXIST  && ok "GitHub token configured"   || warn "GitHub token not found"
    echo
    if ! ask_yn "Upgrade existing installation?" "Y"; then
        echo "Aborted."
        exit 0
    fi
    UPGRADE=true
fi

# Stop existing units before upgrade
if $UPGRADE && $UNITS_EXIST; then
    step "Stopping existing units"
    systemctl --user stop susui-push.path 2>/dev/null && ok "Stopped susui-push.path" || true
    systemctl --user stop susui-push.timer 2>/dev/null && ok "Stopped susui-push.timer" || true
    systemctl --user stop susui-push.service 2>/dev/null || true
fi

# ── Interactive prompts ──────────────────────────────────────────────
step "Configuration"

# 1. Flake reference
echo
printf "  ${DIM}Which flake should susui monitor?${RESET}\n"
while true; do
    ask "Flake ref" "$D_FLAKE_REF" FLAKE_REF
    if [ -n "$FLAKE_REF" ]; then break; fi
    err "Flake ref is required"
done

# ── Flake introspection ───────────────────────────────────────────
step "Inspecting flake"

FLAKE_JSON=""
_flake_err=$(mktemp)
trap 'rm -f "$_flake_err"' EXIT
if command -v jq &>/dev/null; then
    if FLAKE_JSON=$(nix flake metadata "$FLAKE_REF" --json --no-write-lock-file 2>"$_flake_err"); then
        ok "Retrieved flake metadata"

        # List all inputs
        INPUT_NAMES=$(echo "$FLAKE_JSON" | jq -r '.locks.nodes | keys[] | select(. != "root")')

        # Display discovered inputs
        if [ -n "$INPUT_NAMES" ]; then
            printf "  ${DIM}Discovered inputs:${RESET}\n"
            while IFS= read -r iname; do
                itype=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].locked.type // .locks.nodes[$n].original.type // "unknown"')
                if [ "$itype" = "github" ]; then
                    iowner=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.owner // "?"')
                    irepo=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.repo // "?"')
                    ihost=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.host // ""')
                    if [ -n "$ihost" ]; then
                        printf "    ${GREEN}%s${RESET} (%s) → github:%s:%s/%s\n" "$iname" "$itype" "$ihost" "$iowner" "$irepo"
                    else
                        printf "    ${GREEN}%s${RESET} (%s) → github:%s/%s\n" "$iname" "$itype" "$iowner" "$irepo"
                    fi
                else
                    iurl=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.url // "?"')
                    printf "    %s (%s) → %s\n" "$iname" "$itype" "$iurl"
                fi
            done <<< "$INPUT_NAMES"
        fi

        # Pick best input: prefer "src" (any type), else first github input
        DETECTED_INPUT="" DETECTED_OWNER="" DETECTED_REPO="" DETECTED_TYPE="" DETECTED_HOST=""
        FIRST_GH_INPUT="" FIRST_GH_OWNER="" FIRST_GH_REPO="" FIRST_GH_HOST=""

        # parse_git_url <url> — extract owner/repo/host from a github git URL
        # Handles: ssh://git@host/owner/repo, git+ssh://…, https://host/owner/repo,
        #          git@host:owner/repo
        parse_git_url() {
            local url="$1" host="" path=""
            # Strip trailing .git
            url="${url%.git}"
            if [[ "$url" =~ ^(git\+ssh|ssh|https?)://([^/]*@)?([^/:]+)(:[0-9]+)?/(.+)$ ]]; then
                host="${BASH_REMATCH[3]}"
                path="${BASH_REMATCH[5]}"
            elif [[ "$url" =~ ^[^@]+@([^:]+):(.+)$ ]]; then
                host="${BASH_REMATCH[1]}"
                path="${BASH_REMATCH[2]}"
            fi
            if [ -n "$path" ]; then
                # path = owner/repo (possibly with extra segments — take first two)
                GIT_URL_OWNER="${path%%/*}"
                GIT_URL_REPO="${path#*/}"; GIT_URL_REPO="${GIT_URL_REPO%%/*}"
                GIT_URL_HOST="$host"
                return 0
            fi
            return 1
        }

        while IFS= read -r iname; do
            itype=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].locked.type // .locks.nodes[$n].original.type // "unknown"')
            if [ "$iname" = "src" ]; then
                DETECTED_INPUT="src"
                DETECTED_TYPE="$itype"
                if [ "$itype" = "github" ]; then
                    DETECTED_OWNER=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.owner // ""')
                    DETECTED_REPO=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.repo // ""')
                    DETECTED_HOST=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.host // ""')
                else
                    # git/git+ssh — parse URL for owner/repo/host
                    local_url=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.url // .locks.nodes[$n].locked.url // ""')
                    if [ -n "$local_url" ] && parse_git_url "$local_url"; then
                        DETECTED_OWNER="$GIT_URL_OWNER"
                        DETECTED_REPO="$GIT_URL_REPO"
                        DETECTED_HOST="$GIT_URL_HOST"
                        # git+ssh to a github host is effectively "github" for status purposes
                        DETECTED_TYPE="github"
                    fi
                fi
            fi
            if [ "$itype" = "github" ] && [ -z "$FIRST_GH_INPUT" ]; then
                FIRST_GH_INPUT="$iname"
                FIRST_GH_OWNER=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.owner // ""')
                FIRST_GH_REPO=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.repo // ""')
                FIRST_GH_HOST=$(echo "$FLAKE_JSON" | jq -r --arg n "$iname" '.locks.nodes[$n].original.host // ""')
            fi
        done <<< "$INPUT_NAMES"

        # Fall back to first github input if "src" not found
        if [ -z "$DETECTED_INPUT" ] && [ -n "$FIRST_GH_INPUT" ]; then
            DETECTED_INPUT="$FIRST_GH_INPUT"; DETECTED_OWNER="$FIRST_GH_OWNER"
            DETECTED_REPO="$FIRST_GH_REPO"; DETECTED_TYPE="github"; DETECTED_HOST="$FIRST_GH_HOST"
        fi

        # Apply detected values as defaults (only if not already set from existing config)
        if [ -n "$DETECTED_INPUT" ]; then
            if [ -n "$DETECTED_HOST" ] && [ "$DETECTED_HOST" != "github.com" ]; then
                ok "Auto-detected input: $DETECTED_INPUT (github:$DETECTED_HOST:$DETECTED_OWNER/$DETECTED_REPO)"
            elif [ -n "$DETECTED_OWNER" ]; then
                ok "Auto-detected input: $DETECTED_INPUT (github:$DETECTED_OWNER/$DETECTED_REPO)"
            else
                ok "Auto-detected input: $DETECTED_INPUT (type: $DETECTED_TYPE)"
            fi
            [ -z "$D_STATUS_INPUT" ] && D_STATUS_INPUT="$DETECTED_INPUT"
            [ -z "$D_STATUS_TYPE" ] && D_STATUS_TYPE="$DETECTED_TYPE"
            [ -z "$D_STATUS_OWNER" ] && D_STATUS_OWNER="$DETECTED_OWNER"
            [ -z "$D_STATUS_REPO" ] && D_STATUS_REPO="$DETECTED_REPO"
            [ -z "$D_DASH_OWNER" ] && D_DASH_OWNER="$DETECTED_OWNER"
            # Set GHE host if not github.com
            if [ -n "$DETECTED_HOST" ] && [ "$DETECTED_HOST" != "github.com" ]; then
                [ -z "$D_STATUS_HOST" ] && D_STATUS_HOST="$DETECTED_HOST"
                [ -z "$D_DASH_HOST" ] && D_DASH_HOST="$DETECTED_HOST"
            fi
        fi
    else
        warn "Could not read flake metadata — skipping auto-detection"
        # Show the last meaningful line of the nix error
        if [ -s "$_flake_err" ]; then
            _last_err=$(grep -v '^\s*$' "$_flake_err" | tail -1)
            [ -n "$_last_err" ] && printf "  ${DIM}%s${RESET}\n" "$_last_err"
        fi
    fi
else
    warn "jq not found — skipping flake introspection (install jq for auto-detection)"
fi

# 2. susui binary path
ask "Path to susui binary" "$D_BIN" SUSUI_BIN

# 3. Dashboard push
echo
DASH_ENABLED=false
DASH_OWNER="" DASH_REPO="" DASH_BRANCH="gh-pages"
DASH_HOST="" DASH_CNAME="" DASH_MESSAGE=""

if ask_yn "Push build dashboard to GitHub Pages?" "${D_DASH_ENABLED:-Y}"; then
    DASH_ENABLED=true
    ask "Dashboard repo owner" "$D_DASH_OWNER" DASH_OWNER
    ask "Dashboard repo name" "$D_DASH_REPO" DASH_REPO
    ask "Dashboard branch" "$D_DASH_BRANCH" DASH_BRANCH
    if ask_yn "Configure advanced dashboard options?" "N"; then
        ask "GitHub Enterprise host (blank for github.com)" "$D_DASH_HOST" DASH_HOST
        ask "Custom CNAME for Pages" "$D_DASH_CNAME" DASH_CNAME
        ask "Custom commit message" "$D_DASH_MESSAGE" DASH_MESSAGE
    fi
fi

# 4. Status push
echo
STATUS_ENABLED=false
STATUS_INPUT="src" STATUS_TYPE="github" STATUS_OWNER="" STATUS_REPO=""
STATUS_METHOD="commit_status" STATUS_CONTEXT="nix-build/local"
STATUS_CHECK_NAME="" STATUS_TARGET_URL="" STATUS_HOST=""

if ask_yn "Push commit statuses to GitHub?" "${D_STATUS_ENABLED:-Y}"; then
    STATUS_ENABLED=true
    ask "Status input" "${D_STATUS_INPUT:-src}" STATUS_INPUT
    ask "Status type (github/git)" "${D_STATUS_TYPE:-github}" STATUS_TYPE
    ask "Status repo owner" "$D_STATUS_OWNER" STATUS_OWNER
    ask "Status repo name" "$D_STATUS_REPO" STATUS_REPO
    ask "Status method (commit_status/check_run)" "$D_STATUS_METHOD" STATUS_METHOD
    ask "Status context" "$D_STATUS_CONTEXT" STATUS_CONTEXT
    if ask_yn "Configure advanced status options?" "N"; then
        ask "GitHub Enterprise host (blank for github.com)" "$D_STATUS_HOST" STATUS_HOST
        ask "Check run name" "$D_STATUS_CHECK_NAME" STATUS_CHECK_NAME
        ask "Target URL" "$D_STATUS_TARGET_URL" STATUS_TARGET_URL
    fi
fi

# 5. GitHub token
echo
WRITE_TOKEN=true
if $TOKEN_EXIST; then
    if ask_yn "Keep existing GitHub token?" "Y"; then
        WRITE_TOKEN=false
    fi
fi

GH_TOKEN=""
if $WRITE_TOKEN; then
    while true; do
        read -rsp "  GitHub token (input hidden): " GH_TOKEN
        echo
        if [ -n "$GH_TOKEN" ]; then break; fi
        err "Token is required"
    done
fi

# 6. Lingering
echo
ENABLE_LINGER=false
LINGER_STATE=$(loginctl show-user "$USER" -p Linger 2>/dev/null | cut -d= -f2 || echo "no")
if [ "$LINGER_STATE" != "yes" ]; then
    if ask_yn "Enable lingering so units run after logout?" "Y"; then
        ENABLE_LINGER=true
    fi
fi

# ── Installation ─────────────────────────────────────────────────────
step "Installing"

# 1. Create directories
mkdir -p "$CONFIG_DIR" "$SYSTEMD_DIR"
ok "Created config directories"

# 2. Copy push script
cp "$PUSH_SCRIPT" "$CONFIG_DIR/susui-push.sh"
chmod +x "$CONFIG_DIR/susui-push.sh"
ok "Installed susui-push.sh"

# 3. Write push.env
{
    echo "# susui push configuration (generated by susui-setup)"
    echo "SUSUI_FLAKE_REF=$FLAKE_REF"
    echo "SUSUI_BIN=$SUSUI_BIN"
    if $DASH_ENABLED; then
        echo ""
        echo "# Dashboard push"
        echo "SUSUI_DASHBOARD_OWNER=$DASH_OWNER"
        echo "SUSUI_DASHBOARD_REPO=$DASH_REPO"
        echo "SUSUI_DASHBOARD_BRANCH=$DASH_BRANCH"
        [ -n "$DASH_HOST" ]    && echo "SUSUI_DASHBOARD_HOST=$DASH_HOST"
        [ -n "$DASH_CNAME" ]   && echo "SUSUI_DASHBOARD_CNAME=$DASH_CNAME"
        [ -n "$DASH_MESSAGE" ] && echo "SUSUI_DASHBOARD_MESSAGE=$DASH_MESSAGE"
    fi
    if $STATUS_ENABLED; then
        echo ""
        echo "# Status push"
        echo "SUSUI_STATUS_INPUT=$STATUS_INPUT"
        echo "SUSUI_STATUS_TYPE=$STATUS_TYPE"
        echo "SUSUI_STATUS_OWNER=$STATUS_OWNER"
        echo "SUSUI_STATUS_REPO=$STATUS_REPO"
        echo "SUSUI_STATUS_METHOD=$STATUS_METHOD"
        echo "SUSUI_STATUS_CONTEXT=$STATUS_CONTEXT"
        [ -n "$STATUS_CHECK_NAME" ]  && echo "SUSUI_STATUS_CHECK_NAME=$STATUS_CHECK_NAME"
        [ -n "$STATUS_TARGET_URL" ]  && echo "SUSUI_STATUS_TARGET_URL=$STATUS_TARGET_URL"
        [ -n "$STATUS_HOST" ]        && echo "SUSUI_STATUS_HOST=$STATUS_HOST"
    fi
} > "$PUSH_ENV"
chmod 600 "$PUSH_ENV"
ok "Wrote push.env (mode 600)"

# 4. Write github-token.env
if $WRITE_TOKEN; then
    echo "GITHUB_TOKEN=$GH_TOKEN" > "$TOKEN_ENV"
    chmod 600 "$TOKEN_ENV"
    ok "Wrote github-token.env (mode 600)"
else
    ok "Kept existing github-token.env"
fi

# 5. Write systemd units
cat > "$PATH_UNIT" << 'UNIT'
[Unit]
Description=Watch Nix store DB for changes to trigger susui push

[Path]
PathModified=/nix/var/nix/db/db.sqlite
Unit=susui-push.timer
TriggerLimitBurst=5
TriggerLimitIntervalSec=60

[Install]
WantedBy=default.target
UNIT
ok "Wrote susui-push.path"

cat > "$TIMER_UNIT" << 'UNIT'
[Unit]
Description=Debounce timer for susui push

[Timer]
OnActiveSec=30s
Persistent=false
UNIT
ok "Wrote susui-push.timer"

# Capture current PATH so the service can find nix-installed binaries
_svc_path="$PATH"
cat > "$SERVICE_UNIT" << UNIT
[Unit]
Description=Push susui build status and dashboard to GitHub
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=PATH=${_svc_path}
EnvironmentFile=%h/.config/susui/push.env
EnvironmentFile=%h/.config/susui/github-token.env
ExecStart=%h/.config/susui/susui-push.sh
SyslogIdentifier=susui-push
TimeoutStartSec=600
MemoryMax=4G
Nice=10
IOSchedulingClass=idle
UNIT
ok "Wrote susui-push.service"

# 6. Reload and enable
systemctl --user daemon-reload
ok "Reloaded systemd"

systemctl --user enable --now susui-push.path
ok "Enabled susui-push.path"

# 7. Lingering
if $ENABLE_LINGER; then
    loginctl enable-linger "$USER"
    ok "Enabled lingering for $USER"
fi

# ── Verification ─────────────────────────────────────────────────────
step "Verification"

# susui binary
if command -v "$SUSUI_BIN" &>/dev/null; then
    ok "susui binary found: $(command -v "$SUSUI_BIN")"
else
    warn "susui binary '$SUSUI_BIN' not found on PATH"
fi

# Config permissions
for f in "$PUSH_ENV" "$TOKEN_ENV"; do
    if [ -f "$f" ]; then
        perms=$(stat -c %a "$f" 2>/dev/null || stat -f %Lp "$f" 2>/dev/null)
        if [ "$perms" = "600" ]; then
            ok "$(basename "$f") permissions: $perms"
        else
            warn "$(basename "$f") permissions: $perms (expected 600)"
        fi
    fi
done

# Push script
if [ -x "$CONFIG_DIR/susui-push.sh" ]; then
    ok "Push script is executable"
else
    err "Push script is missing or not executable"
fi

# Path unit status
if systemctl --user is-active susui-push.path &>/dev/null; then
    ok "susui-push.path is active"
else
    warn "susui-push.path is not active"
fi

# Lingering
LINGER_STATE=$(loginctl show-user "$USER" -p Linger 2>/dev/null | cut -d= -f2 || echo "unknown")
if [ "$LINGER_STATE" = "yes" ]; then
    ok "Lingering enabled"
else
    warn "Lingering not enabled (units won't run after logout)"
fi

# Optional test run
echo
if ask_yn "Run a test push now?" "N"; then
    step "Running test push"
    echo
    systemctl --user start susui-push.service
    sleep 2
    journalctl --user -u susui-push.service --no-pager -n 20
fi

# ── Summary ──────────────────────────────────────────────────────────
printf "\n${GREEN}╭─────────────────────────────────────────────────────╮${RESET}\n"
printf "${GREEN}│${RESET} ${BOLD}susui push units installed successfully${RESET}             ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}                                                     ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}  View logs:                                         ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}    journalctl --user -u susui-push.service -f       ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}                                                     ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}  Manual trigger:                                    ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}    systemctl --user start susui-push.service        ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}                                                     ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}  Reconfigure:                                       ${GREEN}│${RESET}\n"
printf "${GREEN}│${RESET}    susui-setup                                      ${GREEN}│${RESET}\n"
printf "${GREEN}╰─────────────────────────────────────────────────────╯${RESET}\n"
