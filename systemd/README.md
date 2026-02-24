> **Quick setup:** After `nix profile install`, run `susui-setup` for interactive installation.
> The manual steps below are equivalent to what the setup script does automatically.

# Automatic Status & Dashboard Push via systemd

Automatically pushes GitHub commit statuses and the build dashboard to GitHub Pages whenever the Nix store changes.

Uses a **path → timer → service** pattern: the path unit watches the Nix DB, the timer debounces rapid changes into a single 30-second window, and the service runs a wrapper script that skips no-op pushes by hashing scan output.

## Installation

### 1. Create config directory and copy files

```bash
mkdir -p ~/.config/susui
cp systemd/susui-push.sh ~/.config/susui/
cp systemd/susui-push.conf.example ~/.config/susui/push.env
chmod +x ~/.config/susui/susui-push.sh
```

### 2. Edit push.env

```bash
$EDITOR ~/.config/susui/push.env
```

Set `SUSUI_FLAKE_REF` to your flake, the `SUSUI_DASHBOARD_*` variables for the GitHub Pages target, and the `SUSUI_STATUS_*` variables for commit status reporting. See the comments in the example file for all available options.

The wrapper script generates a `susui.yaml` automatically from these env vars — no separate config file needed. If you already have a `susui.yaml` with both sections configured, you can use `SUSUI_WORKING_DIR` instead (see the example file).

### 3. Create github-token.env

```bash
echo 'GITHUB_TOKEN=ghp_your_token_here' > ~/.config/susui/github-token.env
chmod 600 ~/.config/susui/github-token.env
```

### 4. Install systemd units

```bash
mkdir -p ~/.config/systemd/user
cp systemd/susui-push.path systemd/susui-push.timer systemd/susui-push.service \
   ~/.config/systemd/user/
```

### 5. Enable and start

```bash
systemctl --user daemon-reload
systemctl --user enable --now susui-push.path
```

## Verification

Check that the path unit is watching:

```bash
systemctl --user status susui-push.path
```

Manually trigger a push to test:

```bash
systemctl --user start susui-push.service
```

View logs:

```bash
journalctl --user -u susui-push.service -f
```

Running the service twice in a row should show "Build state unchanged, skipping push." on the second run.

After running `nix build` on your flake, within ~30 seconds you should see the timer activate and the service run. Check:

```bash
systemctl --user status susui-push.timer
```

## Headless / unattended operation

For servers or CI machines where you're not logged in interactively, enable lingering so your user units run after logout:

```bash
loginctl enable-linger $USER
```

## WAL note

SQLite in WAL mode may write to `db.sqlite-wal` without modifying `db.sqlite` itself. If testing shows that the path unit doesn't trigger on some Nix operations, add a second watch line by editing `~/.config/systemd/user/susui-push.path`:

```ini
[Path]
PathModified=/nix/var/nix/db/db.sqlite
PathModified=/nix/var/nix/db/db.sqlite-wal
```

Then reload: `systemctl --user daemon-reload && systemctl --user restart susui-push.path`

## Using with other repos

The default configuration watches the susui repo itself. To monitor a different flake, edit `~/.config/susui/push.env`:

- `SUSUI_FLAKE_REF` — your flake path (e.g. `/home/user/myproject`), GitHub URL (e.g. `github:MyOrg/myrepo`), or git+ssh URL

**Dashboard push** (`SUSUI_DASHBOARD_*`):
- `OWNER` / `ORG` — repo owner or GitHub Enterprise org
- `REPO` — target repo for the dashboard
- `BRANCH` — branch to push to (default: `gh-pages`)
- `HOST` — hostname for GitHub Enterprise
- `CNAME` — custom domain for GitHub Pages
- `MESSAGE` — custom commit message

**Status push** (`SUSUI_STATUS_*`):
- `INPUT` — which flake input's rev to report against (default: `self`)
- `TYPE` — `github` or `git` (default: `github`)
- `OWNER` / `ORG` — repo owner or GitHub Enterprise org
- `REPO` — target repo for status
- `METHOD` — `commit_status` or `check_run` (default: `commit_status`)
- `CONTEXT` — context label for commit statuses
- `CHECK_NAME` — name for check runs
- `TARGET_URL` — URL the status links to
- `HOST` — hostname for GitHub Enterprise

The path unit always watches the same Nix DB regardless of which flake you're tracking — any Nix store change triggers a scan, and the wrapper script's hash comparison ensures only relevant changes result in a push.
