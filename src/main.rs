mod collector;
mod dashboard;
mod models;

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
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a nix flake and display build information
    Scan {
        /// Flake reference (e.g. ".", "github:owner/repo", "/path/to/flake")
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Only evaluate (dry-run), don't actually build
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Include overrides (name=flake-uri pairs)
        #[arg(long, value_name = "NAME=URI")]
        r#override: Vec<String>,
    },

    /// Start the web dashboard
    Serve {
        /// Flake reference to monitor
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Only evaluate, don't build
        #[arg(long)]
        dry_run: bool,

        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },

    /// Show flake metadata and inputs
    Info {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Build a specific derivation
    Build {
        /// Flake reference
        #[arg(default_value = ".")]
        flake_ref: String,

        /// Derivation attribute path
        #[arg(short, long)]
        attr: String,

        /// Override inputs (name=flake-uri pairs)
        #[arg(long, value_name = "NAME=URI")]
        r#override: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone)]
struct AppState {
    builds: Arc<Mutex<Vec<Build>>>,
    metadata: Arc<Mutex<Option<FlakeMetadata>>>,
    flake_ref: String,
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

    match cli.command {
        Commands::Scan {
            flake_ref,
            dry_run,
            json,
            r#override,
        } => cmd_scan(&flake_ref, dry_run, json, &r#override),

        Commands::Serve {
            flake_ref,
            port,
            dry_run,
            bind,
        } => cmd_serve(&flake_ref, port, dry_run, &bind).await,

        Commands::Info { flake_ref, json } => cmd_info(&flake_ref, json),

        Commands::Build {
            flake_ref,
            attr,
            r#override,
            json,
        } => cmd_build(&flake_ref, &attr, &r#override, json),
    }
}

fn cmd_scan(flake_ref: &str, dry_run: bool, json_output: bool, _overrides: &[String]) -> Result<()> {
    let (metadata, builds) = collector::collect_all(flake_ref, dry_run)?;
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
            };
            let ov_mark = if !build.override_inputs.is_empty() {
                format!(" ⚑{}", build.override_inputs.len())
            } else {
                String::new()
            };
            println!(
                "│  {} {} {} {}{}",
                icon,
                build.status,
                truncate(&build.derivation, 35),
                build.duration,
                ov_mark
            );
        }
        println!("╰───────────────────────────────────────────────╯");
    }

    // Exit with non-zero if any builds failed
    if stats.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn cmd_serve(flake_ref: &str, port: u16, dry_run: bool, bind: &str) -> Result<()> {
    tracing::info!(flake_ref, port, "Starting sus ui dashboard");

    // Initial data collection
    let (metadata, builds) = match collector::collect_all(flake_ref, dry_run) {
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
    match collector::collect_all(&state.flake_ref, true) {
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

fn cmd_info(flake_ref: &str, json_output: bool) -> Result<()> {
    let metadata = collector::collect_flake_metadata(flake_ref)?;

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
            println!("│  {} ({}) → {}", input.name, input.input_type, truncate(&input.url, 30));
            if let Some(rev) = &input.locked_rev {
                println!("│    locked: {}", &rev[..12.min(rev.len())]);
            }
        }
        println!("╰───────────────────────────────────────────────╯");
    }
    Ok(())
}

fn cmd_build(flake_ref: &str, attr: &str, overrides: &[String], json_output: bool) -> Result<()> {
    let parsed_overrides: Vec<(String, String)> = overrides
        .iter()
        .filter_map(|s| {
            let parts: Vec<&str> = s.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect();

    let build = collector::build_derivation(flake_ref, attr, &parsed_overrides, 1);

    if json_output {
        println!("{}", serde_json::to_string_pretty(&build)?);
    } else {
        let icon = match build.status {
            BuildStatus::Passed => "✓",
            BuildStatus::Failed => "✕",
            _ => "?",
        };
        println!("{} {} — {} ({})", icon, build.derivation, build.status, build.duration);
        if !build.log.is_empty() {
            println!("── log ──");
            for line in &build.log {
                println!("  {:>3} │ {}", line.n, line.text);
            }
        }
    }

    if build.status == BuildStatus::Failed {
        std::process::exit(1);
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
