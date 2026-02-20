# sus ui — nix builds dashboard

A Rust CLI tool and web dashboard for inspecting Nix flake builds, derivation status, override inputs, and build logs. Supports live web dashboard, static site generation for GitHub Pages, and GitHub status push.

**sus ui is read-only** — it evaluates and introspects the nix store to show build results but never triggers builds itself. Commands shown in the dashboard are informational, allowing you to reproduce builds manually if needed.

## Features

- **Scan** — discover all flake outputs and evaluate them (no builds triggered)
- **Serve** — live web dashboard with auto-refresh
- **Generate** — static HTML dashboard for GitHub Pages (no Jekyll)
- **Info** — inspect flake metadata and resolved inputs
- **Push Status** — push build results to GitHub as commit statuses or check runs using nix store context

## Quick Start

```bash
# Build with nix
nix build github:SusSystems/susui

# Or with cargo
cargo install --path .
```

## Usage

```bash
# Show flake metadata and inputs
susui info .

# Scan all outputs (evaluates derivations, introspects store)
susui scan .

# Start live dashboard
susui serve . --port 3000

# Generate static site for GitHub Pages
susui generate . --output _site

# Push build status to GitHub (uses nix store context, no builds triggered)
susui push-status . --config susui.yaml
```

## Static Site Generation

Generate a self-contained HTML dashboard that can be deployed to GitHub Pages:

```bash
susui generate . --output _site
```

This creates:
- `_site/index.html` — full dashboard with embedded data
- `_site/.nojekyll` — disables Jekyll processing
- `_site/api/builds.json` — build data as JSON
- `_site/api/stats.json` — aggregated statistics
- `_site/api/metadata.json` — flake metadata

Deploy with GitHub Actions:

```yaml
- name: Generate dashboard
  run: susui generate . --output _site
- name: Deploy to Pages
  uses: peaceiris/actions-gh-pages@v3
  with:
    github_token: ${{ secrets.GITHUB_TOKEN }}
    publish_dir: ./_site
```

## Configuration

Create a `susui.yaml` in your project root:

```yaml
# Input filters — control which builds are displayed
filters:
  allow:
    - inputs: ["src"]
      type: github
      owner: my-org
    - inputs: ["src"]
      type: git
      host: github.ibm.com
      org: my-internal-org
  deny:
    - inputs: ["src"]
      type: github
      owner: my-org
      repo: experiments

# GitHub status push — report build results to PRs (uses nix store context)
status_push:
  - input: src
    type: github
    owner: my-org
    repo: my-app
    method: commit_status      # or "check_run"
    context: "nix-build/local"
  - input: src
    type: git
    host: github.ibm.com
    org: my-org
    repo: my-service
    method: check_run
    check_name: "Nix Build"
```

### Filter Rules

Filters control which builds appear on the dashboard based on resolved flake inputs.

- **Allow filters** — only show builds matching at least one rule
- **Deny filters** — hide builds matching any rule
- Rules specify which `inputs` to check (by name) and match on `type`, `owner`/`org`, `repo`, `host`

### Status Push

Push build results to GitHub as commit statuses or check runs. Status is derived from nix store evaluation context — no builds are triggered by this command. Requires `GITHUB_TOKEN` environment variable with appropriate permissions:

| Method | Required Permission |
|--------|---------------------|
| `commit_status` | `repo:status` or **Commit statuses: Write** |
| `check_run` | `checks:write` or **Checks: Write** |

## Data Source Hints

The dashboard includes interactive data source hints — hover any element to see the exact `nix`, `git`, and shell commands used to source that data. These commands are informational, showing how to reproduce or inspect the data yourself. sus ui never triggers builds.

## Building with Nix

```bash
# Build
nix build .#susui

# Development shell
nix develop

# Run checks
nix flake check
```

## License

MIT
