use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::{DashboardPushTarget, StatusPushTarget};
use crate::models::{Build, BuildStatus};

/// Result of a status push
#[derive(Debug, Clone, Serialize)]
pub struct PushResult {
    pub target: String,
    pub sha: String,
    pub state: String,
    pub success: bool,
    pub error: Option<String>,
}

/// GitHub API state for commit statuses
fn build_status_to_gh_state(status: &BuildStatus) -> &'static str {
    match status {
        BuildStatus::Passed => "success",
        BuildStatus::Failed => "failure",
        BuildStatus::Running => "pending",
        BuildStatus::Pending => "pending",
        BuildStatus::Skipped => "success",
        BuildStatus::Unknown => "pending",
    }
}

/// GitHub API conclusion for check runs
fn build_status_to_conclusion(status: &BuildStatus) -> &'static str {
    match status {
        BuildStatus::Passed => "success",
        BuildStatus::Failed => "failure",
        BuildStatus::Running => "neutral",
        BuildStatus::Pending => "neutral",
        BuildStatus::Skipped => "cancelled",
        BuildStatus::Unknown => "neutral",
    }
}

/// Build the API base URL for a target
fn api_base(target: &StatusPushTarget) -> String {
    if let Some(host) = &target.host {
        format!("https://{}/api/v3", host)
    } else {
        "https://api.github.com".to_string()
    }
}

/// Get the owner string from a target
fn target_owner(target: &StatusPushTarget) -> &str {
    target
        .owner
        .as_deref()
        .or(target.org.as_deref())
        .unwrap_or("unknown")
}

/// Expand template variables in context/check_name
fn expand_template(template: &str, build: &Build) -> String {
    template
        .replace("{derivation}", &build.derivation)
        .replace("{flakeRef}", &build.flake_ref)
}

/// Push a commit status to GitHub
async fn push_commit_status(
    client: &reqwest::Client,
    token: &str,
    target: &StatusPushTarget,
    build: &Build,
    sha: &str,
) -> Result<()> {
    let base = api_base(target);
    let owner = target_owner(target);
    let url = format!("{}/repos/{}/{}/statuses/{}", base, owner, target.repo, sha);

    let context = expand_template(&target.context, build);
    let description = format!(
        "Build {} in {}",
        build.status, build.duration
    );

    #[derive(Serialize)]
    struct StatusPayload {
        state: String,
        context: String,
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
    }

    let payload = StatusPayload {
        state: build_status_to_gh_state(&build.status).to_string(),
        context,
        description,
        target_url: target.target_url.clone(),
    };

    let resp = client
        .post(&url)
        .header("Authorization", format!("token {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "susui/0.1")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Failed to push status to {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub API returned {}: {}", status, body);
    }

    Ok(())
}

/// Create or update a check run on GitHub
async fn push_check_run(
    client: &reqwest::Client,
    token: &str,
    target: &StatusPushTarget,
    build: &Build,
    sha: &str,
) -> Result<()> {
    let base = api_base(target);
    let owner = target_owner(target);
    let url = format!("{}/repos/{}/{}/check-runs", base, owner, target.repo);

    let check_name = target
        .check_name
        .as_deref()
        .unwrap_or("Nix Build");
    let name = expand_template(check_name, build);

    let is_complete = matches!(
        build.status,
        BuildStatus::Passed | BuildStatus::Failed | BuildStatus::Skipped
    );

    let log_tail: String = build
        .log
        .iter()
        .rev()
        .take(50)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let payload = serde_json::json!({
        "name": name,
        "head_sha": sha,
        "status": if is_complete { "completed" } else { "in_progress" },
        "conclusion": if is_complete { Some(build_status_to_conclusion(&build.status)) } else { None },
        "output": {
            "title": format!("Build {}", build.status),
            "summary": format!("{} — {} ({})", build.derivation, build.status, build.duration),
            "text": if log_tail.is_empty() { None } else { Some(&log_tail) },
        }
    });

    let resp = client
        .post(&url)
        .header("Authorization", format!("token {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "susui/0.1")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Failed to create check run at {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub API returned {}: {}", status, body);
    }

    Ok(())
}

/// Push build status for all configured targets
pub async fn push_status(
    targets: &[StatusPushTarget],
    builds: &[Build],
    resolved_inputs: &std::collections::HashMap<String, String>, // input_name -> rev
) -> Vec<PushResult> {
    let token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if token.is_empty() {
        tracing::warn!("GITHUB_TOKEN not set, skipping status push");
        return vec![];
    }

    let client = reqwest::Client::new();
    let mut results = Vec::new();

    for target in targets {
        // Find the SHA from the resolved input
        let sha = match resolved_inputs.get(&target.input) {
            Some(rev) => rev.clone(),
            None => {
                tracing::warn!(
                    input = target.input,
                    "No resolved revision found for status push target, skipping"
                );
                continue;
            }
        };

        for build in builds {
            let target_desc = format!(
                "{}/{} ({})",
                target_owner(target),
                target.repo,
                target.method
            );

            let result = match target.method.as_str() {
                "check_run" => {
                    push_check_run(&client, &token, target, build, &sha).await
                }
                _ => {
                    push_commit_status(&client, &token, target, build, &sha).await
                }
            };

            let push_result = match result {
                Ok(()) => {
                    tracing::info!(
                        target = target_desc,
                        sha = &sha[..7.min(sha.len())],
                        state = %build.status,
                        "Status pushed"
                    );
                    PushResult {
                        target: target_desc,
                        sha: sha.clone(),
                        state: build.status.to_string(),
                        success: true,
                        error: None,
                    }
                }
                Err(e) => {
                    tracing::error!(
                        target = target_desc,
                        error = %e,
                        "Failed to push status"
                    );
                    PushResult {
                        target: target_desc,
                        sha: sha.clone(),
                        state: build.status.to_string(),
                        success: false,
                        error: Some(e.to_string()),
                    }
                }
            };

            results.push(push_result);
        }
    }

    results
}

/// Extract resolved input revisions from flake metadata
pub fn extract_input_revisions(
    inputs: &[crate::models::FlakeInput],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for input in inputs {
        if let Some(rev) = &input.locked_rev {
            map.insert(input.name.clone(), rev.clone());
        }
    }
    map
}

// ── Git-based dashboard push ────────────────────────────────────────

/// Result of a dashboard push
#[derive(Debug, Clone, Serialize)]
pub struct DashboardPushResult {
    pub repo: String,
    pub branch: String,
    pub commit_sha: String,
    pub files_pushed: usize,
    pub success: bool,
    pub error: Option<String>,
}

/// Get the owner string from a dashboard push target
fn dashboard_owner(target: &DashboardPushTarget) -> &str {
    target
        .owner
        .as_deref()
        .or(target.org.as_deref())
        .unwrap_or("unknown")
}

/// Push dashboard files to a GitHub repo using git.
///
/// Creates a temporary repo, commits the files, and force-pushes to the
/// target branch. Uses git's native protocol which handles large files
/// without hitting API body-size limits.
pub async fn push_dashboard(
    target: &DashboardPushTarget,
    files: Vec<(String, Vec<u8>)>,
) -> DashboardPushResult {
    let token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return DashboardPushResult {
            repo: target.repo.clone(),
            branch: target.branch.clone(),
            commit_sha: String::new(),
            files_pushed: 0,
            success: false,
            error: Some("GITHUB_TOKEN not set".to_string()),
        };
    }

    let owner = dashboard_owner(target);
    let file_count = files.len();

    match push_dashboard_git(target, files, &token).await {
        Ok(commit_sha) => {
            tracing::info!(
                repo = format!("{}/{}", owner, target.repo),
                branch = target.branch,
                sha = &commit_sha[..7.min(commit_sha.len())],
                files = file_count,
                "Dashboard pushed"
            );
            DashboardPushResult {
                repo: format!("{}/{}", owner, target.repo),
                branch: target.branch.clone(),
                commit_sha,
                files_pushed: file_count,
                success: true,
                error: None,
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to push dashboard");
            DashboardPushResult {
                repo: format!("{}/{}", owner, target.repo),
                branch: target.branch.clone(),
                commit_sha: String::new(),
                files_pushed: 0,
                success: false,
                error: Some(e.to_string()),
            }
        }
    }
}

async fn push_dashboard_git(
    target: &DashboardPushTarget,
    files: Vec<(String, Vec<u8>)>,
    token: &str,
) -> Result<String> {
    let owner = dashboard_owner(target);
    let host = target.host.as_deref().unwrap_or("github.com");
    let message = target
        .commit_message
        .as_deref()
        .unwrap_or("Update dashboard");

    // Create temp directory
    let tmp = std::env::temp_dir().join(format!("susui-push-{}", std::process::id()));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    std::fs::create_dir_all(&tmp)?;

    let result =
        push_dashboard_git_inner(&tmp, owner, &target.repo, &target.branch, host, token, message, &files).await;

    // Always clean up
    let _ = std::fs::remove_dir_all(&tmp);

    result
}

#[allow(clippy::too_many_arguments)]
async fn push_dashboard_git_inner(
    dir: &std::path::Path,
    owner: &str,
    repo: &str,
    branch: &str,
    host: &str,
    token: &str,
    message: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String> {
    // Write all files to the temp directory
    for (path, content) in files {
        let file_path = dir.join(path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file_path, content)?;
    }

    // Initialize git repo and commit
    run_git(dir, &["init"]).await?;
    run_git(dir, &["add", "."]).await?;
    run_git(
        dir,
        &[
            "-c", "user.name=susui",
            "-c", "user.email=susui@users.noreply.github.com",
            "commit", "-m", message,
        ],
    )
    .await?;

    // Build remote URL with token auth
    let remote_url = format!(
        "https://x-access-token:{}@{}/{}/{}.git",
        token, host, owner, repo
    );

    // Force-push to target branch
    run_git_sanitized(
        dir,
        &[
            "push", "--force", &remote_url,
            &format!("HEAD:refs/heads/{}", branch),
        ],
        token,
    )
    .await?;

    // Get commit SHA
    let sha = run_git_output(dir, &["rev-parse", "HEAD"]).await?;
    Ok(sha.trim().to_string())
}

async fn run_git(dir: &std::path::Path, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .context("Failed to run git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr
        );
    }

    Ok(())
}

/// Run a git command, sanitizing the token from any error output
async fn run_git_sanitized(
    dir: &std::path::Path,
    args: &[&str],
    token: &str,
) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .context("Failed to run git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let sanitized = stderr.replace(token, "***");
        anyhow::bail!("git push failed: {}", sanitized);
    }

    Ok(())
}

async fn run_git_output(dir: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .context("Failed to run git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
