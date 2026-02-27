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

        for build in &builds {
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
            println!(
                "│  {} {} {} {}{}{}",
                icon,
                build.status,
                truncate(&build.derivation, 35),
                build.duration,
                ov_mark,
                store_mark
            );
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
