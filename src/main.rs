sdfdfdag
mod collector;
mod config;
mod dashboard;
mod github;
mod models;
mod nixdb;

use anyhow::Result;
use axum::{
    extract::State,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use clap::{Parser, Subcommand};
use models::*;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "susui", version, about = "sus ui — nix build dashboard")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to susui.yaml config file
    #[arg(long, global = true)]
    config: Option<String>,
}

/// Parse a flat list of strings into pairs: ["a", "b", "c", "d"] → [("a","b"), ("c","d")]
fn parse_override_inputs(raw: &[String]) -> Vec<(String, String)> {
    raw.chunks_exact(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a nix flake and display build information (evaluation only, no builds triggered)
    Scan {
        /// Flake reference (e.g. ".", "github:owner/repo", "/path/to/flake")
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Start the web dashboard (read-only, no builds triggered)
    Serve {
        /// Flake reference to monitor
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Generate a static HTML dashboard for GitHub Pages (no builds triggered)
    Generate {
        /// Flake reference to scan
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output directory
        #[arg(short, long, default_value = "_site")]
        output: String,

        /// Base path for GitHub Pages (e.g. "/susui" for project pages)
        #[arg(long, default_value = "/")]
        base_path: String,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Show flake metadata and inputs
    Info {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Push build status to GitHub using nix store context (no builds triggered)
    PushStatus {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },

    /// Generate dashboard and push to a GitHub repo via Git Data API
    PushDashboard {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Base path for the dashboard (e.g. "/" or "/susui")
        #[arg(long, default_value = "/")]
        base_path: String,

        /// Override a flake input (repeatable): --override-input NAME URI
        #[arg(long = "override-input", num_args = 2, action = clap::ArgAction::Append)]
        override_input: Vec<String>,
    },
}

#[derive(Clone)]
struct AppState {
    builds: Arc<Mutex<Vec<Build>>>,
    metadata: Arc<Mutex<Option<FlakeMetadata>>>,
    flake_ref: String,
    overrides: Vec<(String, String)>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("susui=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load_or_default(cli.config.as_deref());

    match cli.command {
        Commands::Scan {
            flake_ref,
            json,
            override_input,
        } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_scan(&flake_ref, json, &cfg, &overrides)
        }

        Commands::Serve {
            flake_ref,
            port,
            bind,
            override_input,
        } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_serve(&flake_ref, port, &bind, &overrides).await
        }

        Commands::Generate {
            flake_ref,
            output,
            base_path,
            override_input,
        } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_generate(&flake_ref, &output, &base_path, &cfg, &overrides)
        }

        Commands::Info { flake_ref, json, override_input } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_info(&flake_ref, json, &overrides)
        }

        Commands::PushStatus {
            flake_ref,
            json,
            override_input,
        } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_push_status(&flake_ref, json, &cfg, &overrides).await
        }

        Commands::PushDashboard {
            flake_ref,
            json,
            base_path,
            override_input,
        } => {
            let overrides = parse_override_inputs(&override_input);
            cmd_push_dashboard(&flake_ref, json, &base_path, &cfg, &overrides).await
        }
    }
}

fn cmd_scan(
    flake_ref: &str,
    json_output: bool,
    _cfg: &config::Config,
    overrides: &[(String, String)],
) -> Result<()> {
    let (metadata, builds) = collector::collect_all(flake_ref, overrides)?;
    let stats = BuildStats::from_builds(&builds);

    if json_output {
        let output = serde_json::json!({
            "metadata": metadata,
            "builds": builds,
            "stats": stats,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("╭─ sus ui · nix build scan ────────────────────╮");
        println!("│  Flake: {:<38}│", flake_ref);
        if let Some(desc) = &metadata.description {
            println!("│  Desc:  {:<38}│", truncate(desc, 38));
        }
        println!("├───────────────────────────────────────────────┤");
        println!(
            "│  {} total · {} passed · {} failed · {} running  ",
            stats.all, stats.passed, stats.failed, stats.running
        );
        println!(
            "│  Success rate: {:.0}%  · {} overridden",
            stats.success_rate, stats.overridden
        );
        println!("├───────────────────────────────────────────────┤");

        // Group builds by (commit, branch, flake_ref)
        // Sentinel commits (containing '…') from unresolved historical builds
        // are normalized to group together under a single "unresolved" key per branch.
        let mut groups: Vec<(String, Option<String>, String, Vec<&Build>)> = Vec::new();
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        // Sort: current (non-historical) first, then historical
        let mut sorted_builds: Vec<&Build> = builds.iter().collect();
        sorted_builds.sort_by_key(|b| b.historical);
        for build in &sorted_builds {
            let group_commit = if build.commit.contains('…') {
                "unresolved".to_string()
            } else {
                build.commit.clone()
            };
            let key = format!(
                "{}|{}|{}",
                group_commit,
                build.branch.as_deref().unwrap_or(""),
                build.flake_ref
            );
            if let Some(&idx) = seen.get(&key) {
                groups[idx].3.push(build);
            } else {
                seen.insert(key.clone(), groups.len());
                groups.push((
                    group_commit,
                    build.branch.clone(),
                    build.flake_ref.clone(),
                    vec![build],
                ));
            }
        }

        for (commit, branch, _flake_ref, group_builds) in &groups {
            let passed = group_builds.iter().filter(|b| b.status == BuildStatus::Passed).count();
            let failed = group_builds.iter().filter(|b| b.status == BuildStatus::Failed).count();
            let running = group_builds.iter().filter(|b| b.status == BuildStatus::Running).count();
            let unknown = group_builds.iter().filter(|b| b.status == BuildStatus::Unknown).count();
            let pending = group_builds.iter().filter(|b| b.status == BuildStatus::Pending).count();

            let short_sha = if commit.len() >= 8 { &commit[..8] } else { commit };
            let branch_str = branch.as_deref().unwrap_or("unknown");

            let mut summary_parts = Vec::new();
            if passed > 0 { summary_parts.push(format!("{} passed", passed)); }
            if failed > 0 { summary_parts.push(format!("{} failed", failed)); }
            if running > 0 { summary_parts.push(format!("{} running", running)); }
            if pending > 0 { summary_parts.push(format!("{} pending", pending)); }
            if unknown > 0 { summary_parts.push(format!("{} unknown", unknown)); }
            let summary = summary_parts.join(", ");

            if commit == "0000000000000000000000000000000000000000" || commit.is_empty() {
                println!("│  ■ {} (unknown commit) ({})", short_sha, summary);
            } else if commit == "unresolved" {
                println!("│  ■ (unresolved commits) {} ({})", branch_str, summary);
            } else {
                println!("│  ■ {} {} ({})", short_sha, branch_str, summary);
            }

            let has_subgroups = group_builds.iter().any(|b| b.input_group.is_some());

            if has_subgroups {
                // Sub-group builds by input_group
                let mut subgroups: Vec<(String, Vec<&&Build>)> = Vec::new();
                let mut sg_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for build in group_builds {
                    let key = build.input_group.clone().unwrap_or_else(|| "unknown".to_string());
                    if let Some(&idx) = sg_seen.get(&key) {
                        subgroups[idx].1.push(build);
                    } else {
                        sg_seen.insert(key.clone(), subgroups.len());
                        subgroups.push((key, vec![build]));
                    }
                }

                for (sg_label, sg_builds) in &subgroups {
                    let sg_passed = sg_builds.iter().filter(|b| b.status == BuildStatus::Passed).count();
                    let sg_failed = sg_builds.iter().filter(|b| b.status == BuildStatus::Failed).count();
                    let mut sg_parts = Vec::new();
                    if sg_passed > 0 { sg_parts.push(format!("{} passed", sg_passed)); }
                    if sg_failed > 0 { sg_parts.push(format!("{} failed", sg_failed)); }
                    let sg_other = sg_builds.len() - sg_passed - sg_failed;
                    if sg_other > 0 { sg_parts.push(format!("{} other", sg_other)); }
                    let sg_summary = sg_parts.join(", ");

                    println!("│    ┌ [stdenv:{}] ({})", sg_label, sg_summary);
                    render_dependency_grouped_builds(&sg_builds, "│    │  ", commit);
                    println!("│    └");
                }
            } else {
                render_dependency_grouped_builds_flat(group_builds, "│    ");
            }
        }
        println!("╰───────────────────────────────────────────────╯");
    }

    if stats.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn cmd_serve(flake_ref: &str, port: u16, bind: &str, overrides: &[(String, String)]) -> Result<()> {
    tracing::info!(flake_ref, port, "Starting sus ui dashboard");

    let (metadata, builds) = match collector::collect_all(flake_ref, overrides) {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!("Initial scan failed: {}. Starting with empty data.", e);
            (
                FlakeMetadata {
                    description: None,
                    url: flake_ref.to_string(),
                    resolved_url: flake_ref.to_string(),
                    revision: None,
                    inputs: vec![],
                },
                vec![],
            )
        }
    };

    let state = AppState {
        builds: Arc::new(Mutex::new(builds)),
        metadata: Arc::new(Mutex::new(Some(metadata))),
        flake_ref: flake_ref.to_string(),
        overrides: overrides.to_vec(),
    };

    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/api/builds", get(api_builds))
        .route("/api/stats", get(api_stats))
        .route("/api/metadata", get(api_metadata))
        .route("/api/refresh", get(api_refresh))
        .with_state(state);

    let addr = format!("{}:{}", bind, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    println!("╭─ sus ui ──────────────────────────────╮");
    println!("│                                       │");
    println!("│   Dashboard:  http://{}   │", format_addr(&addr));
    println!("│   API:        http://{}/api│", format_addr(&addr));
    println!("│                                       │");
    println!("│   Monitoring: {:<24}│", truncate(flake_ref, 24));
    println!("│   柵 — cut clean, ship fast.           │");
    println!("╰────────────────────────────────────────╯");

    axum::serve(listener, app).await?;
    Ok(())
}

fn cmd_generate(
    flake_ref: &str,
    output_dir: &str,
    base_path: &str,
    _cfg: &config::Config,
    overrides: &[(String, String)],
) -> Result<()> {
    tracing::info!(flake_ref, output_dir, "Generating static dashboard");

    let (metadata, builds) = collector::collect_all(flake_ref, overrides)?;
    let stats = BuildStats::from_builds(&builds);

    let builds_json = serde_json::to_string(&builds)?;
    let meta_json = serde_json::to_string(&Some(&metadata))?;

    // Generate the static HTML (with API polling disabled)
    let html_content = dashboard::static_dashboard_html(&builds_json, &meta_json);

    // Create output directory
    std::fs::create_dir_all(output_dir)?;

    // Write index.html
    let index_path = std::path::Path::new(output_dir).join("index.html");
    std::fs::write(&index_path, &html_content)?;

    // Write .nojekyll for GitHub Pages
    let nojekyll_path = std::path::Path::new(output_dir).join(".nojekyll");
    std::fs::write(&nojekyll_path, "")?;

    // Write API JSON files for optional static hosting
    let api_dir = std::path::Path::new(output_dir).join("api");
    std::fs::create_dir_all(&api_dir)?;

    std::fs::write(
        api_dir.join("builds.json"),
        serde_json::to_string_pretty(&ApiResponse::success(&builds))?,
    )?;
    std::fs::write(
        api_dir.join("stats.json"),
        serde_json::to_string_pretty(&ApiResponse::success(stats))?,
    )?;
    std::fs::write(
        api_dir.join("metadata.json"),
        serde_json::to_string_pretty(&ApiResponse::success(&metadata))?,
    )?;

    // Write CNAME if base_path is a custom domain
    if base_path.contains('.') && !base_path.starts_with('/') {
        std::fs::write(
            std::path::Path::new(output_dir).join("CNAME"),
            base_path,
        )?;
    }

    println!("╭─ sus ui · static site generated ──────────────╮");
    println!("│                                                │");
    println!("│  Output: {:<38}│", output_dir);
    println!("│  Builds: {:<38}│", builds.len());
    println!("│                                                │");
    println!("│  Files:                                        │");
    println!("│    index.html      — dashboard                 │");
    println!("│    .nojekyll       — disable Jekyll             │");
    println!("│    api/builds.json — build data                │");
    println!("│    api/stats.json  — aggregated stats          │");
    println!("│    api/metadata.json — flake metadata          │");
    println!("│                                                │");
    println!("│  Deploy:                                       │");
    println!("│    gh-pages branch or GitHub Actions            │");
    println!("│                                                │");
    println!("╰────────────────────────────────────────────────╯");

    Ok(())
}

async fn cmd_push_status(
    flake_ref: &str,
    json_output: bool,
    cfg: &config::Config,
    overrides: &[(String, String)],
) -> Result<()> {
    if cfg.status_push.is_empty() {
        anyhow::bail!(
            "No status_push targets configured. Add them to susui.yaml:\n\
             \n\
             status_push:\n\
             \x20 - input: src\n\
             \x20   type: github\n\
             \x20   owner: my-org\n\
             \x20   repo: my-app\n\
             \x20   method: commit_status\n\
             \x20   context: nix-build/local"
        );
    }

    let (metadata, builds) = collector::collect_all(flake_ref, overrides)?;
    let revisions = github::extract_input_revisions(&metadata.inputs);

    let results = github::push_status(&cfg.status_push, &builds, &revisions).await;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for r in &results {
            let icon = if r.success { "✓" } else { "✕" };
            println!(
                "  {} {} → {} ({})",
                icon,
                &r.sha[..7.min(r.sha.len())],
                r.target,
                r.state
            );
            if let Some(err) = &r.error {
                println!("    error: {}", err);
            }
        }
    }

    Ok(())
}

async fn cmd_push_dashboard(
    flake_ref: &str,
    json_output: bool,
    base_path: &str,
    cfg: &config::Config,
    overrides: &[(String, String)],
) -> Result<()> {
    let target = match &cfg.dashboard_push {
        Some(t) => t,
        None => {
            anyhow::bail!(
                "No dashboard_push target configured. Add it to susui.yaml:\n\
                 \n\
                 dashboard_push:\n\
                 \x20 owner: my-org\n\
                 \x20 repo: my-dashboard\n\
                 \x20 branch: gh-pages\n\
                 \x20 # cname: builds.example.com\n\
                 \x20 # commit_message: \"Update dashboard\""
            );
        }
    };

    tracing::info!(flake_ref, "Generating dashboard for push");
    let (metadata, new_builds) = collector::collect_all(flake_ref, overrides)?;

    // Merge with existing builds from the deployed dashboard
    let existing = github::fetch_existing_builds(target).await;
    let max_builds = target.max_builds.unwrap_or(500);
    let builds = github::merge_builds(existing, new_builds, max_builds);

    let stats = BuildStats::from_builds(&builds);

    let builds_json = serde_json::to_string(&builds)?;
    let meta_json = serde_json::to_string(&Some(&metadata))?;

    let html_content = dashboard::static_dashboard_html(&builds_json, &meta_json);

    // Build the file list
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("index.html".to_string(), html_content.into_bytes()),
        (".nojekyll".to_string(), Vec::new()),
        (
            "api/builds.json".to_string(),
            serde_json::to_string_pretty(&ApiResponse::success(&builds))?.into_bytes(),
        ),
        (
            "api/stats.json".to_string(),
            serde_json::to_string_pretty(&ApiResponse::success(stats))?.into_bytes(),
        ),
        (
            "api/metadata.json".to_string(),
            serde_json::to_string_pretty(&ApiResponse::success(&metadata))?.into_bytes(),
        ),
    ];

    // Add CNAME if configured
    if let Some(cname) = &target.cname {
        files.push(("CNAME".to_string(), cname.as_bytes().to_vec()));
    } else if base_path.contains('.') && !base_path.starts_with('/') {
        files.push(("CNAME".to_string(), base_path.as_bytes().to_vec()));
    }

    let result = github::push_dashboard(target, files).await;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.success {
        println!("╭─ sus ui · dashboard pushed ─────────────────╮");
        println!("│                                              │");
        println!("│  Repo:   {:<36}│", result.repo);
        println!("│  Branch: {:<36}│", result.branch);
        println!(
            "│  Commit: {:<36}│",
            &result.commit_sha[..7.min(result.commit_sha.len())]
        );
        println!("│  Files:  {:<36}│", result.files_pushed);
        println!("│                                              │");
        println!("╰──────────────────────────────────────────────╯");
    } else {
        let err = result.error.as_deref().unwrap_or("unknown error");
        anyhow::bail!("Dashboard push failed: {}", err);
    }

    Ok(())
}

async fn serve_dashboard(State(state): State<AppState>) -> impl IntoResponse {
    let builds = state.builds.lock().unwrap().clone();
    let metadata = state.metadata.lock().unwrap().clone();

    let builds_json = serde_json::to_string(&builds).unwrap_or_else(|_| "[]".to_string());
    let meta_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "null".to_string());

    let html_content = dashboard::dashboard_html(&builds_json, &meta_json);

    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(html_content),
    )
}

async fn api_builds(State(state): State<AppState>) -> Json<ApiResponse<Vec<Build>>> {
    let builds = state.builds.lock().unwrap().clone();
    Json(ApiResponse::success(builds))
}

async fn api_stats(State(state): State<AppState>) -> Json<ApiResponse<BuildStats>> {
    let builds = state.builds.lock().unwrap().clone();
    let stats = BuildStats::from_builds(&builds);
    Json(ApiResponse::success(stats))
}

async fn api_metadata(
    State(state): State<AppState>,
) -> Json<ApiResponse<Option<FlakeMetadata>>> {
    let metadata = state.metadata.lock().unwrap().clone();
    Json(ApiResponse::success(metadata))
}

async fn api_refresh(State(state): State<AppState>) -> Json<ApiResponse<BuildStats>> {
    tracing::info!("Refreshing build data");
    match collector::collect_all(&state.flake_ref, &state.overrides) {
        Ok((meta, builds)) => {
            let stats = BuildStats::from_builds(&builds);
            *state.builds.lock().unwrap() = builds;
            *state.metadata.lock().unwrap() = Some(meta);
            Json(ApiResponse::success(stats))
        }
        Err(e) => {
            tracing::error!("Refresh failed: {}", e);
            let builds = state.builds.lock().unwrap().clone();
            Json(ApiResponse::success(BuildStats::from_builds(&builds)))
        }
    }
}

fn cmd_info(flake_ref: &str, json_output: bool, overrides: &[(String, String)]) -> Result<()> {
    let metadata = collector::collect_flake_metadata(flake_ref, overrides)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        println!("╭─ Flake Info ─────────────────────────────────╮");
        println!("│  URL: {:<40}│", truncate(&metadata.url, 40));
        if let Some(desc) = &metadata.description {
            println!("│  Desc: {:<39}│", truncate(desc, 39));
        }
        if let Some(rev) = &metadata.revision {
            println!("│  Rev:  {:<39}│", &rev[..7.min(rev.len())]);
        }
        println!("├─ Inputs ──────────────────────────────────────┤");
        for input in &metadata.inputs {
            println!(
                "│  {} ({}) → {}",
                input.name,
                input.input_type,
                truncate(&input.url, 30)
            );
            if let Some(rev) = &input.locked_rev {
                println!("│    locked: {}", &rev[..12.min(rev.len())]);
            }
        }
        println!("╰───────────────────────────────────────────────╯");
    }
    Ok(())
}

/// Format a build line for the TUI scan output
fn format_build_line(build: &Build) -> String {
    let icon = match build.status {
        BuildStatus::Passed => "✓",
        BuildStatus::Failed => "✕",
        BuildStatus::Running => "↻",
        BuildStatus::Pending => "◦",
        BuildStatus::Skipped => "—",
        BuildStatus::Unknown => "?",
    };
    let ov_mark = if !build.override_inputs.is_empty() {
        format!(" ⚑{}", build.override_inputs.len())
    } else {
        String::new()
    };
    let store_mark = if build.in_store { "" } else { " ◌" };
    let alias_mark = if build.is_alias { " ≡" } else { "" };
    format!(
        "{} {} {} {}{}{}{}",
        icon,
        build.status,
        truncate(&build.derivation, 33),
        build.duration,
        ov_mark,
        store_mark,
        alias_mark
    )
}

/// Extract the package leaf name that a check depends on.
/// `checks.x86_64-linux.build-gsk_cms` → `gsk_cms`
/// `checks.x86_64-linux.test-gsk_ssl_bvt` → `gsk_ssl_bvt`
fn check_depends_on_leaf(attr: &str) -> Option<String> {
    if !attr.starts_with("checks.") {
        return None;
    }
    let leaf = attr.split('.').next_back()?;
    leaf.strip_prefix("build-")
        .or_else(|| leaf.strip_prefix("test-"))
        .map(|s| s.to_string())
}

/// Group builds by dependency: packages first, with their dependent checks grouped
/// underneath, then independent checks, devShells, formatter, and other outputs.
///
/// Returns a Vec of (group_label, builds) where group_label is:
///   - Some("packages.x86_64-linux.gsk_cms") for a package + its checks
///   - None for ungrouped builds
fn dependency_group_builds<'a, B: std::ops::Deref<Target = Build>>(
    builds: &'a [B],
) -> Vec<(Option<&'a str>, Vec<&'a Build>)> {
    use std::collections::{HashMap, HashSet};

    // Collect package drvs and leaf→attr mapping
    let mut pkg_drv_to_attr: HashMap<&str, &str> = HashMap::new();
    let mut pkg_leaf_to_drv: HashMap<String, HashSet<&str>> = HashMap::new();

    for b in builds {
        let b: &Build = b;
        if b.derivation.starts_with("packages.") {
            if let Some(ref drv) = b.drv_path {
                pkg_drv_to_attr.insert(drv.as_str(), &b.derivation);
                let leaf = b.derivation.split('.').next_back().unwrap_or("");
                if leaf != "default" {
                    pkg_leaf_to_drv.entry(leaf.to_string()).or_default().insert(drv.as_str());
                }
            }
        }
    }

    // Figure out which builds go under which package
    // Key: package derivation attr → Vec of builds (package + checks)
    let mut grouped: Vec<(Option<&str>, Vec<&Build>)> = Vec::new();
    let mut used: HashSet<usize> = HashSet::new();

    // First pass: emit packages with their dependent checks
    for (i, b) in builds.iter().enumerate() {
        let b_ref: &Build = b;
        if !b_ref.derivation.starts_with("packages.") {
            continue;
        }
        if b_ref.is_alias {
            continue;
        }
        if used.contains(&i) {
            continue;
        }

        let pkg_attr = &b_ref.derivation;
        let pkg_leaf = pkg_attr.split('.').next_back().unwrap_or("");
        if pkg_leaf == "default" || pkg_leaf == "full" {
            continue; // handle meta-packages later
        }

        let mut group_builds: Vec<&Build> = vec![b_ref];
        used.insert(i);

        // Find checks that depend on this package (by drv_path match or leaf name)
        for (j, cb) in builds.iter().enumerate() {
            let cb_ref: &Build = cb;
            if used.contains(&j) || !cb_ref.derivation.starts_with("checks.") {
                continue;
            }

            let matches = if let (Some(ref c_drv), Some(ref p_drv)) = (&cb_ref.drv_path, &b_ref.drv_path) {
                c_drv == p_drv
            } else if let Some(dep_leaf) = check_depends_on_leaf(&cb_ref.derivation) {
                dep_leaf == pkg_leaf
            } else {
                false
            };

            if matches {
                group_builds.push(cb_ref);
                used.insert(j);
            }
        }

        grouped.push((Some(pkg_attr.as_str()), group_builds));
    }

    // Second pass: remaining builds (independent checks, devShells, formatter, aliases, meta-packages)
    for (i, b) in builds.iter().enumerate() {
        if used.contains(&i) {
            continue;
        }
        used.insert(i);
        let b_ref: &Build = b;
        grouped.push((None, vec![b_ref]));
    }

    grouped
}

/// Render dependency-grouped builds for a subgroup (with commit annotation).
/// In subgroups, every build shows its commit (or "commit not found").
fn render_dependency_grouped_builds(builds: &[&&Build], prefix: &str, _group_commit: &str) {
    let derefs: Vec<&Build> = builds.iter().map(|b| **b).collect();
    let dep_groups = dependency_group_builds(&derefs);

    for (pkg_label, group_builds) in &dep_groups {
        if let Some(_label) = pkg_label {
            // Package group: package builds first, then checks indented
            let pkg_builds: Vec<&&Build> = group_builds.iter()
                .filter(|b| b.derivation.starts_with("packages."))
                .collect();
            let check_builds: Vec<&&Build> = group_builds.iter()
                .filter(|b| !b.derivation.starts_with("packages."))
                .collect();

            for build in &pkg_builds {
                let commit_mark = subgroup_commit_annotation(&build.commit);
                println!("{}{}{}", prefix, format_build_line(build), commit_mark);
            }
            for build in &check_builds {
                let commit_mark = subgroup_commit_annotation(&build.commit);
                println!("{}  └ {}{}", prefix, format_build_line(build), commit_mark);
            }
        } else {
            for build in group_builds {
                let commit_mark = subgroup_commit_annotation(&build.commit);
                println!("{}{}{}", prefix, format_build_line(build), commit_mark);
            }
        }
    }
}

/// Render dependency-grouped builds for the flat (non-subgrouped) view.
fn render_dependency_grouped_builds_flat(builds: &[&Build], prefix: &str) {
    let dep_groups = dependency_group_builds(builds);

    for (pkg_label, group_builds) in &dep_groups {
        if let Some(_) = pkg_label {
            let pkg_builds: Vec<&&Build> = group_builds.iter()
                .filter(|b| b.derivation.starts_with("packages."))
                .collect();
            let dep_builds: Vec<&&Build> = group_builds.iter()
                .filter(|b| !b.derivation.starts_with("packages."))
                .collect();

            for build in &pkg_builds {
                println!("{}{}", prefix, format_build_line(build));
            }
            for build in &dep_builds {
                println!("{}  └ {}", prefix, format_build_line(build));
            }
        } else {
            for build in group_builds {
                println!("{}{}", prefix, format_build_line(build));
            }
        }
    }
}

/// Show a commit annotation when the build's commit differs from the group commit.
#[allow(dead_code)]
fn commit_annotation(build_commit: &str, group_commit: &str) -> String {
    if build_commit == group_commit {
        return String::new();
    }
    // Unresolved historical builds have commit like "<hash>…<drv_name>"
    if build_commit.contains('…') {
        return " (commit not found)".to_string();
    }
    let short = if build_commit.len() >= 8 { &build_commit[..8] } else { build_commit };
    format!(" @{}", short)
}

/// Show a commit annotation for every build within a subgroup.
/// Always displays the commit (or "commit not found" for unresolved builds).
fn subgroup_commit_annotation(build_commit: &str) -> String {
    if build_commit.contains('…') {
        return " (commit not found)".to_string();
    }
    if build_commit == "0000000000000000000000000000000000000000" || build_commit.is_empty() {
        return " (commit not found)".to_string();
    }
    let short = if build_commit.len() >= 8 { &build_commit[..8] } else { build_commit };
    format!(" @{}", short)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn format_addr(addr: &str) -> String {
    if addr.len() < 20 {
        format!("{:<20}", addr)
    } else {
        addr.to_string()
    }
}
