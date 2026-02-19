# sus ui — nix build dashboard

柵 — **cut clean, ship fast.**

A Rust CLI tool and web dashboard for monitoring nix flake builds. Scans your flake outputs, runs or evaluates builds, and serves a real-time dashboard with data-source hints.

## Features

- **Flake scanning** — discovers all derivation outputs from `nix flake show`
- **Build execution** — runs `nix build` with timing, log capture, and exit code tracking
- **Override inputs** — supports `--override-input` for testing against forks/branches
- **Web dashboard** — serves the saku-themed UI with real build data and auto-refresh
- **JSON API** — `/api/builds`, `/api/stats`, `/api/metadata`, `/api/refresh`
- **Data source hints** — every UI element shows the nix/git/shell commands that source its data
- **CLI output** — `scan`, `build`, `info` commands with human-readable and `--json` output

## Install

```bash
# With nix flakes
nix run github:SusSystems/susui -- serve .

# Or build and install
nix build github:SusSystems/susui
./result/bin/susui --help

# Dev shell
nix develop
cargo build --release
```

## Usage

```bash
# Scan all flake outputs (dry-run)
susui scan . --dry-run

# Scan and build, output JSON
susui scan . --json

# Start the web dashboard
susui serve . --port 3000

# Build a specific derivation
susui build . --attr packages.x86_64-linux.default

# Build with override
susui build . --attr packages.x86_64-linux.default \
  --override nixpkgs=github:NixOS/nixpkgs/nixos-unstable

# Show flake metadata
susui info .
susui info github:NixOS/nixpkgs --json
```

## API

When running `susui serve`, the following endpoints are available:

| Endpoint | Description |
|---|---|
| `GET /` | Dashboard HTML |
| `GET /api/builds` | All build records |
| `GET /api/stats` | Aggregate statistics |
| `GET /api/metadata` | Flake metadata and inputs |
| `GET /api/refresh` | Trigger a rescan |

## Dev

```bash
nix develop
cargo test
cargo clippy
cargo run -- serve . --port 3000
```

## License

MIT
