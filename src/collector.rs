use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::models::*;

/// Discover nix binary on PATH or in common locations
fn nix_bin() -> String {
    // Check PATH first
    if let Ok(output) = Command::new("which").arg("nix").output() {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }
    // Common locations
    for path in &[
        "/nix/var/nix/profiles/default/bin/nix",
        "/root/.nix-profile/bin/nix",
        "/home/claude/.nix-profile/bin/nix",
    ] {
        if Path::new(path).exists() {
            return path.to_string();
        }
    }
    // Scan /nix/store for nix binaries
    if let Ok(entries) = std::fs::read_dir("/nix/store") {
        for entry in entries.flatten() {
            let nix_path = entry.path().join("bin/nix");
            if nix_path.exists() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("-nix-") && !name.contains("determinate") {
                    return nix_path.to_string_lossy().to_string();
                }
            }
        }
    }
    "nix".to_string()
}

/// Run a command and return stdout
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run: {} {:?}", cmd, args))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!(
            "Command failed ({}): {}",
            output.status,
            stderr
        ))
    }
}

/// Run a command and capture both stdout+stderr, regardless of exit code
fn run_cmd_full(cmd: &str, args: &[&str]) -> (bool, String, String) {
    match Command::new(cmd).args(args).output() {
        Ok(output) => (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ),
        Err(e) => (false, String::new(), format!("Failed to execute: {}", e)),
    }
}

/// Extract local git directory from a flake ref.
/// Returns None for remote flake refs (github:, git+ssh://, etc.)
fn git_dir_from_flake_ref(flake_ref: &str) -> Option<String> {
    if flake_ref == "." || flake_ref.starts_with("./") || flake_ref.starts_with('/') {
        Some(flake_ref.to_string())
    } else if let Some(rest) = flake_ref.strip_prefix("path:") {
        Some(rest.to_string())
    } else if let Some(rest) = flake_ref.strip_prefix("git+file://") {
        // git+file:///absolute → /absolute
        Some(rest.to_string())
    } else {
        // git+file:./relative → ./relative
        flake_ref.strip_prefix("git+file:").map(|rest| rest.to_string())
    }
}

/// Get the current git branch in a directory
fn git_branch(dir: &str) -> Option<String> {
    run_cmd("git", &["-C", dir, "branch", "--show-current"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get the current git commit in a directory
fn git_commit(dir: &str) -> Option<String> {
    run_cmd("git", &["-C", dir, "rev-parse", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
}

/// Check if the git working tree is dirty
fn git_is_dirty(dir: &str) -> bool {
    let (ok, stdout, _) = run_cmd_full("git", &["-C", dir, "status", "--porcelain", "--untracked-files=no"]);
    ok && !stdout.trim().is_empty()
}

/// Given a commit hash, return which branch contains it.
/// Prefers the current branch (via `git merge-base --is-ancestor`),
/// falls back to `git branch --contains`.
#[allow(dead_code)]
fn git_branch_containing(dir: &str, commit: &str) -> Option<String> {
    // Check if the current branch contains this commit
    if let Some(current) = git_branch(dir) {
        let (ok, _, _) = run_cmd_full("git", &["-C", dir, "merge-base", "--is-ancestor", commit, &current]);
        if ok {
            return Some(current);
        }
    }

    // Fall back to git branch --contains
    let (ok, stdout, _) = run_cmd_full("git", &["-C", dir, "branch", "--contains", commit]);
    if !ok {
        return None;
    }
    // Parse output: lines like "* main" or "  feature-branch"
    stdout
        .lines()
        .map(|l| l.trim_start_matches('*').trim().to_string())
        .find(|l| !l.is_empty())
}

/// Parse a git URL string into `(forge_url, owner, repo)`.
///
/// Works on raw URL strings from git remotes, nix flake metadata, or flake inputs.
/// Handles nix-specific prefixes (`git+ssh://`, `git+https://`), query params (`?rev=…`),
/// and fragment suffixes (`#attr`).
///
/// Supported URL formats:
/// - `https://HOST/owner/repo.git` → `https://HOST`
/// - `git@HOST:owner/repo.git` → `https://HOST`
/// - `ssh://git@HOST/owner/repo.git` → `https://HOST`
/// - `git+ssh://git@HOST/owner/repo.git` → `https://HOST`
/// - `git+https://HOST/owner/repo.git` → `https://HOST`
/// - `github:owner/repo` → `https://github.com` (nix shorthand)
fn parse_git_url(raw_url: &str) -> (Option<String>, Option<String>, Option<String>) {
    // Strip nix git+ prefix: git+ssh:// → ssh://, git+https:// → https://
    let url = raw_url
        .strip_prefix("git+")
        .unwrap_or(raw_url);

    // Strip fragment (#attr) and query params (?rev=abc&ref=main)
    let url = url.split('#').next().unwrap_or(url);
    let url = url.split('?').next().unwrap_or(url);
    let url = url.trim();

    // Nix shorthand: github:owner/repo
    if let Some(rest) = url.strip_prefix("github:") {
        let rest = rest.trim_end_matches(".git").trim_end_matches('/');
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return (
                Some("https://github.com".to_string()),
                Some(parts[0].to_string()),
                Some(parts[1].to_string()),
            );
        }
        return (None, None, None);
    }

    // ssh://git@HOST/owner/repo.git
    if let Some(rest) = url.strip_prefix("ssh://") {
        // rest = "git@HOST/owner/repo.git" or "git@HOST:port/owner/repo.git"
        let after_at = rest.find('@').map(|i| &rest[i + 1..]).unwrap_or(rest);
        // Split host from path at first '/'
        if let Some(slash_pos) = after_at.find('/') {
            let host = &after_at[..slash_pos];
            // Remove port if present (e.g. "HOST:port")
            let host_no_port = host.split(':').next().unwrap_or(host);
            let path = after_at[slash_pos + 1..].trim_end_matches(".git").trim_end_matches('/');
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 2 {
                return (
                    Some(format!("https://{}", host_no_port)),
                    Some(parts[0].to_string()),
                    Some(parts[1].to_string()),
                );
            }
        }
        return (None, None, None);
    }

    // git@HOST:owner/repo.git
    if let Some(after_at) = url.strip_prefix("git@") {
        if let Some(colon_pos) = after_at.find(':') {
            let host = &after_at[..colon_pos];
            let path = after_at[colon_pos + 1..].trim_end_matches(".git").trim_end_matches('/');
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 2 {
                return (
                    Some(format!("https://{}", host)),
                    Some(parts[0].to_string()),
                    Some(parts[1].to_string()),
                );
            }
        }
        return (None, None, None);
    }

    // https://HOST/owner/repo.git (with optional auth token stripping)
    if url.starts_with("https://") || url.starts_with("http://") {
        let cleaned = if let Some(at_pos) = url.find('@') {
            // Strip auth: https://x-access-token:TOKEN@HOST/... → https://HOST/...
            let scheme_end = url.find("://").unwrap() + 3;
            format!("{}{}", &url[..scheme_end], &url[at_pos + 1..])
        } else {
            url.to_string()
        };
        // cleaned = "https://HOST/owner/repo.git"
        let scheme_end = cleaned.find("://").unwrap() + 3;
        let after_scheme = &cleaned[scheme_end..];
        // Split: HOST / owner / repo.git
        let segments: Vec<&str> = after_scheme.trim_end_matches(".git").trim_end_matches('/').splitn(3, '/').collect();
        if segments.len() == 3 {
            let host = segments[0];
            return (
                Some(format!("{}://{}", &cleaned[..cleaned.find("://").unwrap()], host)),
                Some(segments[1].to_string()),
                Some(segments[2].to_string()),
            );
        }
        return (None, None, None);
    }

    (None, None, None)
}

/// Get the git remote forge URL, owner, and repo.
///
/// Returns `(forge_url, owner, repo)` where `forge_url` is like `"https://github.com"`.
/// Fetches the origin remote URL via `git remote get-url origin` and delegates to `parse_git_url()`.
fn git_remote_info(dir: &str) -> (Option<String>, Option<String>, Option<String>) {
    let url = match run_cmd("git", &["-C", dir, "remote", "get-url", "origin"]) {
        Ok(u) => u.trim().to_string(),
        Err(_) => return (None, None, None),
    };
    parse_git_url(&url)
}

/// Resolved git information for a flake, from local git or flake metadata.
struct ResolvedGitInfo {
    commit: String,
    branch: Option<String>,
    forge_url: Option<String>,
    owner: Option<String>,
    repo: Option<String>,
    dirty: bool,
}

/// Resolve git info for a flake, trying local git first then falling back to flake metadata.
///
/// Priority order:
/// 1. Local git (existing behavior for local refs like `.`, `path:`, `git+file:`)
/// 2. `src` input from flake metadata (primary build subject for `--override-input src` workflows)
/// 3. Flake's own revision from metadata
fn resolve_git_info(flake_ref: &str, metadata: &FlakeMetadata) -> ResolvedGitInfo {
    // 1. Try local git first (existing behavior for local refs)
    if let Some(dir) = git_dir_from_flake_ref(flake_ref) {
        if let Some(commit) = git_commit(&dir) {
            let branch = git_branch(&dir);
            let (forge_url, owner, repo) = git_remote_info(&dir);
            let dirty = git_is_dirty(&dir);
            return ResolvedGitInfo { commit, branch, forge_url, owner, repo, dirty };
        }
    }

    // 2. Remote ref: prefer "src" input (primary build subject)
    if let Some(src) = metadata.inputs.iter().find(|i| i.name == "src") {
        if let Some(rev) = &src.locked_rev {
            let (forge_url, owner, repo) = parse_git_url(&src.url);
            let branch = src.locked_ref.clone();
            return ResolvedGitInfo {
                commit: rev.clone(),
                branch,
                forge_url,
                owner,
                repo,
                dirty: false,
            };
        }
    }

    // 3. Final fallback: flake's own revision
    let commit = metadata.revision.clone().unwrap_or_else(|| "0".repeat(40));
    let (forge_url, owner, repo) = parse_git_url(&metadata.resolved_url);
    ResolvedGitInfo {
        commit,
        branch: None,
        forge_url,
        owner,
        repo,
        dirty: false,
    }
}

/// Parse nix flake metadata JSON
fn parse_flake_metadata(json_str: &str) -> Result<FlakeMetadata> {
    let v: serde_json::Value = serde_json::from_str(json_str)?;

    let description = v.get("description").and_then(|d| d.as_str()).map(String::from);
    let url = v
        .get("url")
        .and_then(|u| u.as_str())
        .unwrap_or(".")
        .to_string();
    let resolved_url = v
        .get("resolvedUrl")
        .or_else(|| v.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or(".")
        .to_string();
    let revision = v.get("revision").and_then(|r| r.as_str()).map(String::from);

    let mut inputs = Vec::new();
    if let Some(locks) = v.get("locks").and_then(|l| l.get("nodes")) {
        if let Some(obj) = locks.as_object() {
            for (name, node) in obj {
                if name == "root" {
                    continue;
                }
                let locked = node.get("locked");
                let original = node.get("original");

                let input_type = locked
                    .and_then(|l| l.get("type"))
                    .or_else(|| original.and_then(|o| o.get("type")))
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                let input_url = if input_type == "github" {
                    let owner = original
                        .and_then(|o| o.get("owner"))
                        .and_then(|o| o.as_str())
                        .unwrap_or("?");
                    let repo = original
                        .and_then(|o| o.get("repo"))
                        .and_then(|o| o.as_str())
                        .unwrap_or("?");
                    format!("github:{}/{}", owner, repo)
                } else {
                    original
                        .and_then(|o| o.get("url"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("?")
                        .to_string()
                };

                let locked_rev = locked
                    .and_then(|l| l.get("rev"))
                    .and_then(|r| r.as_str())
                    .map(String::from);

                let locked_ref = locked
                    .and_then(|l| l.get("ref"))
                    .or_else(|| original.and_then(|o| o.get("ref")))
                    .and_then(|r| r.as_str())
                    .map(String::from);

                let last_modified = locked
                    .and_then(|l| l.get("lastModified"))
                    .and_then(|t| t.as_i64())
                    .map(|ts| {
                        chrono::DateTime::from_timestamp(ts, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| ts.to_string())
                    });

                inputs.push(FlakeInput {
                    name: name.clone(),
                    input_type,
                    url: input_url,
                    locked_rev,
                    locked_ref,
                    last_modified,
                });
            }
        }
    }

    Ok(FlakeMetadata {
        description,
        url,
        resolved_url,
        revision,
        inputs,
    })
}

/// List flake outputs as attribute paths
fn list_flake_outputs(nix: &str, flake_ref: &str, overrides: &[(String, String)]) -> Result<Vec<String>> {
    let mut args = vec!["flake", "show", flake_ref, "--json", "--no-write-lock-file"];
    append_override_args(&mut args, overrides);
    let output = run_cmd(nix, &args)?;
    let v: serde_json::Value = serde_json::from_str(&output)?;

    let mut attrs = Vec::new();
    collect_attrs(&v, "", &mut attrs);
    Ok(attrs)
}

fn collect_attrs(val: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    if let Some(obj) = val.as_object() {
        // If this node has a "type" key, it's a leaf derivation
        if obj.contains_key("type") {
            if !prefix.is_empty() {
                out.push(prefix.to_string());
            }
            return;
        }
        for (key, child) in obj {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };
            collect_attrs(child, &path, out);
        }
    }
}

/// Collect flake metadata for a given flake reference
pub fn collect_flake_metadata(flake_ref: &str, overrides: &[(String, String)]) -> Result<FlakeMetadata> {
    let nix = nix_bin();
    let mut args = vec!["flake", "metadata", flake_ref, "--json", "--no-write-lock-file"];
    append_override_args(&mut args, overrides);
    let output = run_cmd(&nix, &args)?;
    parse_flake_metadata(&output)
}

/// Build a single derivation and return a Build record.
/// NOTE: This function is retained for testing purposes. The CLI never invokes
/// builds directly — it only evaluates and introspects the nix store.
#[allow(dead_code)]
fn build_derivation(
    flake_ref: &str,
    attr: &str,
    overrides: &[(String, String)],
    id: u64,
    git_info: &ResolvedGitInfo,
) -> Build {
    let nix = nix_bin();
    let target = format!("{}#{}", flake_ref, attr);

    let mut args = vec!["build", &target, "--no-link", "-L"];
    let override_strs: Vec<String> = overrides
        .iter()
        .map(|(name, uri)| format!("{}={}", name, uri))
        .collect();
    for ov in &override_strs {
        args.push("--override-input");
        let parts: Vec<&str> = ov.splitn(2, '=').collect();
        args.push(parts[0]);
        args.push(parts.get(1).unwrap_or(&""));
    }

    // Run with timing
    let start = Instant::now();
    let (success, stdout, stderr) = run_cmd_full(&nix, &args);
    let elapsed = start.elapsed();

    let duration = format_duration(elapsed);
    let combined_output = format!("{}{}", stdout, stderr);

    // Parse log lines, filtering nix noise
    let log_lines = make_log_lines(&combined_output);

    let branch = git_info.branch.clone();
    let commit = git_info.commit.clone();
    let forge_url = git_info.forge_url.clone();
    let owner = git_info.owner.clone();
    let repo = git_info.repo.clone();
    let dirty = git_info.dirty;

    let status = if success {
        BuildStatus::Passed
    } else {
        BuildStatus::Failed
    };

    let override_inputs: Vec<OverrideInput> = overrides
        .iter()
        .map(|(name, uri)| {
            let (input_type, ov_owner, ov_repo, git_ref) = parse_flake_uri(uri);
            OverrideInput {
                input_name: name.clone(),
                input_type,
                owner: ov_owner,
                repo: ov_repo,
                git_ref,
                pr: None,
            }
        })
        .collect();

    Build {
        id,
        derivation: attr.to_string(),
        status,
        duration,
        time: "just now".to_string(),
        branch,
        commit,
        owner,
        repo,
        forge_url,
        flake_ref: flake_ref.to_string(),
        pr: None,
        override_inputs,
        log: log_lines,
        drv_path: None,
        store_path: None,
        in_store: success,
        historical: false,
        dirty,
        is_alias: false,
    }
}

/// Evaluate a derivation and return a Build record by introspecting the nix store.
///
/// Uses `nix path-info --derivation` to resolve the .drv path, then:
///   1. Checks if the output path exists in the store (`nix path-info`)
///   2. Retrieves cached build logs via multiple strategies
///   3. Falls back to `nix derivation show` for structured recipe info
///
/// The status is determined by store presence:
///   - Output in store → Passed (previously built successfully)
///   - Output NOT in store, eval succeeded → Unknown (not yet built)
///   - Eval failed → Failed
fn eval_derivation(flake_ref: &str, attr: &str, overrides: &[(String, String)], id: u64, git_info: &ResolvedGitInfo) -> Build {
    let nix = nix_bin();
    let target = format!("{}#{}", flake_ref, attr);

    let mut args = vec!["path-info", "--derivation", &target];
    append_override_args(&mut args, overrides);

    let start = Instant::now();
    let (success, stdout, stderr) = run_cmd_full(&nix, &args);
    let elapsed = start.elapsed();

    let duration = format_duration(elapsed);

    // The derivation store path is the meaningful output
    let drv_path = stdout.trim().to_string();

    let branch = git_info.branch.clone();
    let commit = git_info.commit.clone();
    let forge_url = git_info.forge_url.clone();
    let owner = git_info.owner.clone();
    let repo = git_info.repo.clone();
    let dirty = git_info.dirty;

    // If we got a .drv path back, the evaluation succeeded even if
    // the exit code was non-zero due to stderr warnings.
    let eval_ok = success || drv_path.starts_with("/nix/store/");

    if !eval_ok {
        // Evaluation itself failed — show the filtered error output
        let combined = format!("{}{}", stdout, stderr);
        let log_lines = make_log_lines(&combined);
        let override_inputs: Vec<OverrideInput> = overrides
            .iter()
            .map(|(name, uri)| {
                let (input_type, ov_owner, ov_repo, git_ref) = parse_flake_uri(uri);
                OverrideInput {
                    input_name: name.clone(),
                    input_type,
                    owner: ov_owner,
                    repo: ov_repo,
                    git_ref,
                    pr: None,
                }
            })
            .collect();
        return Build {
            id,
            derivation: attr.to_string(),
            status: BuildStatus::Failed,
            duration,
            time: "just now".to_string(),
            branch,
            commit,
            owner,
            repo,
            forge_url,
            flake_ref: flake_ref.to_string(),
            pr: None,
            override_inputs,
            log: log_lines,
            drv_path: None,
            store_path: None,
            in_store: false,
            historical: false,
            dirty,
            is_alias: false,
        };
    }

    // Check if the output path exists in the store
    let (store_path, in_store) = check_output_in_store(&nix, &target, &drv_path, overrides);

    // Retrieve logs with multiple strategies
    let (log_lines, has_build_log) = enrich_eval_logs(&nix, &target, &drv_path, store_path.as_deref());

    // Determine status: passed if output is in store, failed if a build log
    // exists (meaning the build was attempted and failed), unknown otherwise.
    let status = if in_store {
        BuildStatus::Passed
    } else if has_build_log {
        BuildStatus::Failed
    } else {
        BuildStatus::Unknown
    };

    let override_inputs: Vec<OverrideInput> = overrides
        .iter()
        .map(|(name, uri)| {
            let (input_type, ov_owner, ov_repo, git_ref) = parse_flake_uri(uri);
            OverrideInput {
                input_name: name.clone(),
                input_type,
                owner: ov_owner,
                repo: ov_repo,
                git_ref,
                pr: None,
            }
        })
        .collect();

    Build {
        id,
        derivation: attr.to_string(),
        status,
        duration,
        time: "just now".to_string(),
        branch,
        commit,
        owner,
        repo,
        forge_url,
        flake_ref: flake_ref.to_string(),
        pr: None,
        override_inputs,
        log: log_lines,
        drv_path: Some(drv_path),
        store_path,
        in_store,
        historical: false,
        dirty,
        is_alias: false,
    }
}

/// Check whether a derivation's output path exists in the nix store.
/// Returns (store_path, exists).
fn check_output_in_store(nix: &str, target: &str, _drv_path: &str, overrides: &[(String, String)]) -> (Option<String>, bool) {
    // Strategy 1: `nix path-info <target>` — asks for the output, not the .drv
    let mut pi_args = vec!["path-info", target];
    append_override_args(&mut pi_args, overrides);
    let (ok, stdout, _) = run_cmd_full(nix, &pi_args);
    if ok {
        let path = stdout.trim().to_string();
        if !path.is_empty() && path.starts_with("/nix/store/") && !path.ends_with(".drv") {
            return (Some(path), true);
        }
    }

    // Strategy 2: parse outputs from `nix derivation show` and check each
    let (ok, stdout, _) = run_cmd_full(nix, &["derivation", "show", target]);
    if ok {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
            // Nix 2.33+: { "derivations": { "hash-name.drv": { ... } }, "version": 4 }
            // Older nix: { "/nix/store/hash-name.drv": { ... } }
            let drvs_map = parsed
                .get("derivations")
                .and_then(|d| d.as_object())
                .or_else(|| parsed.as_object())
                .cloned()
                .unwrap_or_default();

            for (_drv_key, drv) in &drvs_map {
                // Skip non-object entries (e.g. "version": 4)
                if !drv.is_object() {
                    continue;
                }
                if let Some(outputs) = drv.get("outputs").and_then(|o| o.as_object()) {
                    // Check the "out" output first, then any other
                    for out_name in &["out", "lib", "dev", "bin", "doc"] {
                        if let Some(out_data) = outputs.get(*out_name) {
                            let raw_path = out_data
                                .as_object()
                                .and_then(|o| o.get("path"))
                                .and_then(|p| p.as_str())
                                .unwrap_or("");
                            if raw_path.is_empty() {
                                continue;
                            }
                            // Nix 2.33+ may omit /nix/store/ prefix
                            let full_path = if raw_path.starts_with("/nix/store/") {
                                raw_path.to_string()
                            } else {
                                format!("/nix/store/{}", raw_path)
                            };
                            // Check if this path actually exists
                            let exists = std::path::Path::new(&full_path).exists();
                            return (Some(full_path), exists);
                        }
                    }
                }
            }
        }
    }

    (None, false)
}

/// Enrich eval-mode logs with cached build output or derivation metadata.
///
/// Tries multiple strategies in order:
///   1. `nix log <target>` — flake ref based log lookup
///   2. `nix log <drv>` — derivation path based lookup
///   3. `nix log <output_path>` — output path based lookup
///   4. Direct filesystem lookup in /nix/var/log/nix/drvs/
///   5. Historical log search by derivation name
///   6. `cargo test` fallback for Rust check derivations
fn enrich_eval_logs(nix: &str, target: &str, drv_path: &str, store_path: Option<&str>) -> (Vec<LogLine>, bool) {
    // Strategy 1: try `nix log` with the flake target ref
    if let Some(log_lines) = try_cached_log(nix, target, "flake target") {
        if !log_lines.is_empty() {
            return (log_lines, true);
        }
    }

    // Strategy 2: try `nix log` with the .drv path
    if drv_path.starts_with("/nix/store/") {
        if let Some(log_lines) = try_cached_log(nix, drv_path, "derivation") {
            if !log_lines.is_empty() {
                return (log_lines, true);
            }
        }
    }

    // Strategy 3: try `nix log` with the output store path
    if let Some(out_path) = store_path {
        if let Some(log_lines) = try_cached_log(nix, out_path, "output path") {
            if !log_lines.is_empty() {
                return (log_lines, true);
            }
        }
    }

    // Strategy 4: direct filesystem lookup in /nix/var/log/nix/drvs/.
    // `nix log` can fail to resolve even when the log file physically exists,
    // so look for it directly by constructing the path from the drv hash.
    if let Some(log_lines) = try_direct_log_file(drv_path) {
        if !log_lines.is_empty() {
            return (log_lines, true);
        }
    }

    // Strategy 5: search for a previous build log by derivation name.
    // When the source changes, the drv hash changes but the derivation name
    // (e.g. "susui-0.1.0") stays the same. Look in the nix log cache for any
    // prior build of the same derivation name.
    if let Some(log_lines) = try_historical_log(nix, drv_path) {
        if !log_lines.is_empty() {
            // Combine: show the historical log first, then derivation metadata
            let mut combined = log_lines;
            let separator_n = combined.len() + 1;
            combined.push(LogLine {
                n: separator_n,
                text: String::new(),
                level: "dim".to_string(),
            });
            let meta = derivation_info_lines(nix, target, drv_path);
            for (i, mut line) in meta.into_iter().enumerate() {
                line.n = separator_n + 1 + i;
                combined.push(line);
            }
            return (combined, false);
        }
    }

    // Strategy 6: for check derivations of Rust projects, try `cargo test`
    // to show the actual test output even when no nix log is cached.
    if target.contains("#checks.") {
        if let Some(log_lines) = try_cargo_test_fallback(target) {
            if !log_lines.is_empty() {
                let mut combined = log_lines;
                let sep_n = combined.len() + 1;
                combined.push(LogLine {
                    n: sep_n,
                    text: String::new(),
                    level: "dim".to_string(),
                });
                let meta = derivation_info_lines(nix, target, drv_path);
                for (i, mut line) in meta.into_iter().enumerate() {
                    line.n = sep_n + 1 + i;
                    combined.push(line);
                }
                return (combined, false);
            }
        }
    }

    // Fall back to derivation metadata only
    (derivation_info_lines(nix, target, drv_path), false)
}

/// Try to fetch a cached build log via `nix log`.
fn try_cached_log(nix: &str, log_ref: &str, source_label: &str) -> Option<Vec<LogLine>> {
    let (ok, stdout, _stderr) = run_cmd_full(nix, &["log", log_ref]);
    if !ok || stdout.trim().is_empty() {
        return None;
    }

    let lines = make_log_lines(&stdout);
    if lines.is_empty() {
        return None;
    }

    let short_ref = if log_ref.len() > 60 {
        // Shorten store paths
        log_ref.split('/').next_back().unwrap_or(log_ref)
    } else {
        log_ref
    };

    // Prepend a header so it's clear this is a cached log
    let mut result = vec![LogLine {
        n: 1,
        text: format!("─── cached build log ({}: {}) ───", source_label, short_ref),
        level: "dim".to_string(),
    }];

    for (i, mut line) in lines.into_iter().enumerate() {
        line.n = i + 2;
        result.push(line);
    }

    Some(result)
}

/// Direct filesystem lookup for a build log in /nix/var/log/nix/drvs/.
///
/// `nix log` can fail to resolve even when the compressed log file physically
/// exists on disk. This bypasses `nix log` entirely and reads the bz2 file
/// directly using `bzcat`.
fn try_direct_log_file(drv_path: &str) -> Option<Vec<LogLine>> {
    let drv_basename = drv_path.split('/').next_back()?;
    if drv_basename.len() < 3 {
        return None;
    }

    let prefix = &drv_basename[..2];
    let rest = &drv_basename[2..];
    let log_file = format!("/nix/var/log/nix/drvs/{}/{}.bz2", prefix, rest);
    let log_path = std::path::Path::new(&log_file);

    if !log_path.exists() {
        return None;
    }

    let (ok, stdout, _stderr) = run_cmd_full("bzcat", &[&log_file]);
    if !ok || stdout.trim().is_empty() {
        return None;
    }

    let lines = make_log_lines(&stdout);
    if lines.is_empty() {
        return None;
    }

    let short_drv = if drv_basename.len() > 40 {
        &drv_basename[..40]
    } else {
        drv_basename
    };

    let mut result = vec![LogLine {
        n: 1,
        text: format!("─── cached build log (drv log: {}…) ───", short_drv),
        level: "dim".to_string(),
    }];

    for (i, mut line) in lines.into_iter().enumerate() {
        line.n = i + 2;
        result.push(line);
    }

    Some(result)
}

/// Search /nix/var/log/nix/drvs/ for a previous build log of the same derivation name.
///
/// When the source changes, the nix store hash changes but the derivation name
/// (e.g. "susui-0.1.0") stays the same. This function finds logs from prior builds
/// by matching the name portion of the .drv filename.
///
/// Returns the most recent log (largest file, heuristic) with a header indicating
/// it's a historical log from a prior build.
fn try_historical_log(nix: &str, drv_path: &str) -> Option<Vec<LogLine>> {
    // Extract derivation name: "/nix/store/<hash>-<name>.drv" → "<name>"
    let drv_filename = drv_path.split('/').next_back().unwrap_or("");
    // Remove the leading hash (32 chars + dash)
    let name_with_ext = if drv_filename.len() > 33 && drv_filename.as_bytes()[32] == b'-' {
        &drv_filename[33..]
    } else {
        return None;
    };
    // name_with_ext is like "susui-0.1.0.drv"
    let drv_name = name_with_ext.trim_end_matches(".drv");
    if drv_name.is_empty() {
        return None;
    }

    // Search /nix/var/log/nix/drvs/ for matching log files
    let log_base = std::path::Path::new("/nix/var/log/nix/drvs");
    if !log_base.exists() {
        return None;
    }

    let suffix = format!("-{}.drv.bz2", drv_name);
    let current_hash = &drv_filename[..32];

    let mut best_path: Option<(std::path::PathBuf, u64)> = None;

    if let Ok(subdirs) = std::fs::read_dir(log_base) {
        for subdir_entry in subdirs.flatten() {
            if let Ok(files) = std::fs::read_dir(subdir_entry.path()) {
                for file_entry in files.flatten() {
                    let fname = file_entry.file_name();
                    let fname_str = fname.to_string_lossy();
                    if fname_str.ends_with(&suffix) {
                        // Skip the current drv (already tried in strategy 2)
                        let file_hash = &fname_str[..std::cmp::min(32, fname_str.len())];
                        if file_hash == current_hash {
                            continue;
                        }
                        if let Ok(meta) = file_entry.metadata() {
                            let size = meta.len();
                            // Prefer the largest log file (most build output = most useful)
                            if size > 0
                                && best_path.as_ref().is_none_or(|(_, s)| size > *s)
                            {
                                best_path = Some((file_entry.path(), size));
                            }
                        }
                    }
                }
            }
        }
    }

    let (log_file_path, _) = best_path?;

    // Reconstruct the full /nix/store/ drv path from the log filename
    let log_fname = log_file_path
        .file_name()?
        .to_string_lossy()
        .trim_end_matches(".bz2")
        .to_string();
    let log_dir_name = log_file_path
        .parent()?
        .file_name()?
        .to_string_lossy()
        .to_string();
    let old_drv_path = format!("/nix/store/{}{}", log_dir_name, log_fname);

    // Fetch the log via nix log
    let (ok, stdout, _stderr) = run_cmd_full(nix, &["log", &old_drv_path]);
    if !ok || stdout.trim().is_empty() {
        return None;
    }

    let lines = make_log_lines(&stdout);
    if lines.is_empty() {
        return None;
    }

    let old_hash = &log_fname[..std::cmp::min(8, log_fname.len())];

    let mut result = vec![LogLine {
        n: 1,
        text: format!("─── build log (prior build: {}…) ───", old_hash),
        level: "dim".to_string(),
    }, LogLine {
        n: 2,
        text: "note: source has changed since this log was produced (drv hash differs)".to_string(),
        level: "dim".to_string(),
    }];

    for (i, mut line) in lines.into_iter().enumerate() {
        line.n = i + 3;
        result.push(line);
    }

    Some(result)
}

/// Fallback for Rust check derivations: run `cargo test` in the source directory
/// to capture actual test output when no nix build log is cached.
///
/// This is a best-effort strategy — it only works when:
///   - The flake ref points to a local directory
///   - That directory contains a Cargo.toml
///   - A Rust toolchain is available on PATH
///
/// The output is wrapped with phase markers so the dashboard's BuildLog component
/// can render it with collapsible sections.
fn try_cargo_test_fallback(target: &str) -> Option<Vec<LogLine>> {
    // Extract flake ref from target: ".#checks.x86_64-linux.susui" → "."
    let flake_ref = target.split('#').next().unwrap_or(".");
    let src_dir = git_dir_from_flake_ref(flake_ref)?;

    let cargo_toml = std::path::Path::new(&src_dir).join("Cargo.toml");
    if !cargo_toml.exists() {
        return None;
    }

    // Check that cargo is available
    if Command::new("which").arg("cargo").output().map(|o| !o.status.success()).unwrap_or(true) {
        // Try common locations
        let cargo_path = [
            std::env::var("HOME").unwrap_or_default() + "/.cargo/bin/cargo",
            "/usr/bin/cargo".to_string(),
        ];
        let found = cargo_path.iter().find(|p| std::path::Path::new(p).exists());
        found?;
    }

    // Resolve cargo binary
    let cargo_bin = Command::new("which")
        .arg("cargo")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            let p = format!("{}/.cargo/bin/cargo", home);
            if std::path::Path::new(&p).exists() { Some(p) } else { None }
        })?;

    // Run cargo test (with phase markers for BuildLog component)
    let (_ok, stdout, stderr) = run_cmd_full(
        &cargo_bin,
        &["test", "--color=never"],
    );

    let combined = format!("{}{}", stderr, stdout);
    if combined.trim().is_empty() {
        return None;
    }

    // Build phase-annotated log lines
    let mut lines = Vec::new();
    let mut n = 1;

    // Header
    lines.push(LogLine { n, text: "─── cargo test (live fallback — no cached nix log) ───".to_string(), level: "dim".to_string() });
    n += 1;

    // Separate compilation output from test output
    let mut in_test_section = false;
    lines.push(LogLine { n, text: "Running phase: buildPhase".to_string(), level: "nix".to_string() });
    n += 1;

    for raw_line in combined.lines() {
        if is_nix_noise(raw_line) {
            continue;
        }

        // Detect the transition to test execution
        let trimmed = raw_line.trim();
        if !in_test_section && (trimmed.starts_with("Running ") || trimmed.starts_with("running ")) && trimmed.contains("test") {
            in_test_section = true;
            lines.push(LogLine { n, text: "Running phase: checkPhase".to_string(), level: "nix".to_string() });
            n += 1;
        }

        lines.push(LogLine {
            n,
            text: raw_line.to_string(),
            level: classify_log_line(raw_line),
        });
        n += 1;
    }

    // If we never entered test section, mark the whole thing as checkPhase
    if !in_test_section && !lines.is_empty() {
        // Insert a checkPhase marker after the header
        lines.insert(2, LogLine { n: 0, text: "Running phase: checkPhase".to_string(), level: "nix".to_string() });
    }

    // Renumber
    for (i, line) in lines.iter_mut().enumerate() {
        line.n = i + 1;
    }

    if lines.len() <= 2 {
        return None;
    }

    Some(lines)
}

/// Extract readable derivation info from `nix derivation show`.
fn derivation_info_lines(nix: &str, target: &str, drv_path: &str) -> Vec<LogLine> {
    let (ok, stdout, _stderr) = run_cmd_full(nix, &["derivation", "show", target]);
    if !ok {
        // Last resort: just show the drv path
        return vec![LogLine {
            n: 1,
            text: drv_path.to_string(),
            level: "success".to_string(),
        }];
    }

    let mut lines: Vec<(String, String)> = Vec::new(); // (text, level)

    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
        // Find the derivation object — it's under "derivations" (nix 2.33+) or at the top level
        let drvs = parsed
            .get("derivations")
            .and_then(|d| d.as_object())
            .or_else(|| parsed.as_object());

        if let Some(drvs_map) = drvs {
            for (drv_key, drv) in drvs_map {
                let obj = match drv.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                let env = obj.get("env").and_then(|e| e.as_object());

                // Header
                let name = env
                    .and_then(|e| e.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(drv_key);
                lines.push((
                    format!("─── derivation: {} ───", name),
                    "dim".to_string(),
                ));

                // System & builder
                if let Some(sys) = obj.get("system").and_then(|v| v.as_str()) {
                    lines.push((format!("system:   {}", sys), "info".to_string()));
                }
                if let Some(builder) = obj.get("builder").and_then(|v| v.as_str()) {
                    let short = builder.split('/').next_back().unwrap_or(builder);
                    lines.push((format!("builder:  {}", short), "info".to_string()));
                }

                if let Some(env_map) = env {
                    // Key build metadata
                    for key in &["pname", "version", "src", "cargoDeps", "cargoBuildType"] {
                        if let Some(val) = env_map.get(*key).and_then(|v| v.as_str()) {
                            let display = if val.starts_with("/nix/store/") {
                                val.split('/').next_back().unwrap_or(val)
                            } else {
                                val
                            };
                            lines.push((format!("{:<14}{}", format!("{}:", key), display), "info".to_string()));
                        }
                    }

                    // Build phases (the actual commands)
                    for phase in &["configurePhase", "buildPhase", "checkPhase", "installPhase"] {
                        if let Some(val) = env_map.get(*phase).and_then(|v| v.as_str()) {
                            let val = val.trim();
                            if !val.is_empty() && val != ":" {
                                lines.push((String::new(), "dim".to_string()));
                                lines.push((
                                    format!("─ {} ─", phase),
                                    "nix".to_string(),
                                ));
                                for cmd_line in val.lines() {
                                    let trimmed = cmd_line.trim();
                                    if !trimmed.is_empty() {
                                        lines.push((
                                            format!("  {}", trimmed),
                                            if trimmed.starts_with("runHook") {
                                                "dim".to_string()
                                            } else {
                                                "info".to_string()
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                    }

                    // Build inputs (shortened)
                    for key in &["nativeBuildInputs", "buildInputs"] {
                        if let Some(val) = env_map.get(*key).and_then(|v| v.as_str()) {
                            let deps: Vec<&str> = val
                                .split_whitespace()
                                .filter_map(|p| p.split('/').next_back())
                                .collect();
                            if !deps.is_empty() {
                                lines.push((String::new(), "dim".to_string()));
                                lines.push((
                                    format!("─ {} ({}) ─", key, deps.len()),
                                    "nix".to_string(),
                                ));
                                for dep in &deps {
                                    lines.push((format!("  {}", dep), "dim".to_string()));
                                }
                            }
                        }
                    }
                }

                // Outputs
                if let Some(outputs) = obj.get("outputs").and_then(|o| o.as_object()) {
                    lines.push((String::new(), "dim".to_string()));
                    lines.push(("─ outputs ─".to_string(), "nix".to_string()));
                    for (oname, odata) in outputs {
                        let path = odata
                            .as_object()
                            .and_then(|o| o.get("path"))
                            .and_then(|p| p.as_str())
                            .unwrap_or("?");
                        lines.push((
                            format!("  {}: /nix/store/{}", oname, path),
                            "success".to_string(),
                        ));
                    }
                }

                // Input counts
                if let Some(inputs) = obj.get("inputs").and_then(|i| i.as_object()) {
                    let drv_count = inputs
                        .get("drvs")
                        .and_then(|d| d.as_object())
                        .map(|d| d.len())
                        .unwrap_or(0);
                    let src_count = inputs
                        .get("srcs")
                        .and_then(|s| s.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    lines.push((String::new(), "dim".to_string()));
                    lines.push((
                        format!("inputs: {} derivations, {} sources", drv_count, src_count),
                        "dim".to_string(),
                    ));
                }
            }
        }
    }

    // If parsing failed or produced nothing, at least show the drv path
    if lines.is_empty() {
        lines.push((drv_path.to_string(), "success".to_string()));
    }

    lines
        .into_iter()
        .enumerate()
        .map(|(i, (text, level))| LogLine {
            n: i + 1,
            text,
            level,
        })
        .collect()
}

/// Discover historical builds by querying the nix SQLite database for prior `.drv`
/// files that share the same derivation name as current builds. Includes passed
/// builds (output in store), failed builds (build log exists), and unknown builds
/// (evaluation succeeded but not yet built or in progress).
///
/// Uses the nix DB instead of filesystem scanning for reliability and performance.
fn find_historical_builds(nix: &str, current_builds: &[Build]) -> Vec<Build> {
    // Build a map of derivation names → current drv hashes (to exclude)
    // Also map drv_name → (flake_ref, derivation attr) for building the Build entry
    let mut current_hashes: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    let mut drv_name_info: std::collections::HashMap<String, Vec<(&str, &str)>> =
        std::collections::HashMap::new();

    for build in current_builds {
        let drv_path = match &build.drv_path {
            Some(p) => p,
            None => continue,
        };
        let drv_filename = drv_path.split('/').next_back().unwrap_or("");
        if drv_filename.len() <= 33 || drv_filename.as_bytes()[32] != b'-' {
            continue;
        }
        let name_with_ext = &drv_filename[33..];
        let drv_name = name_with_ext.trim_end_matches(".drv");
        if drv_name.is_empty() {
            continue;
        }
        let hash = &drv_filename[..32];
        current_hashes
            .entry(drv_name.to_string())
            .or_default()
            .insert(hash.to_string());
        drv_name_info
            .entry(drv_name.to_string())
            .or_default()
            .push((&build.flake_ref, &build.derivation));
    }

    if current_hashes.is_empty() {
        return Vec::new();
    }

    // Query the nix DB for matching .drv paths
    let name_patterns: Vec<String> = current_hashes.keys().cloned().collect();
    let db_matches = crate::nixdb::find_drvs_by_name(&name_patterns);

    // Filter out current hashes
    let historical_drvs: Vec<(String, String, String)> = db_matches
        .into_iter()
        .filter(|(_path, name, hash)| {
            current_hashes
                .get(name)
                .map_or(true, |cur| !cur.contains(hash))
        })
        .collect();

    if historical_drvs.is_empty() {
        return Vec::new();
    }

    // Query outputs for all historical drvs
    let drv_paths: Vec<String> = historical_drvs.iter().map(|(p, _, _)| p.clone()).collect();
    let outputs_map = crate::nixdb::find_outputs_for_drvs(&drv_paths);

    let mut historical_builds = Vec::new();

    for (old_drv_path, drv_name, drv_hash) in &historical_drvs {
        let attrs = match drv_name_info.get(drv_name.as_str()) {
            Some(info) => info.clone(),
            None => continue,
        };

        // Check outputs from the DB query
        let db_outputs = outputs_map.get(old_drv_path.as_str());

        // Determine if any output path actually exists in the store
        let found_output: Option<(String, i64)> = db_outputs
            .and_then(|outs| {
                outs.iter()
                    .find(|(path, _)| std::path::Path::new(path).exists())
                    .cloned()
            });

        // Determine status and metadata
        let (status, store_path, in_store, time_str) = if let Some((out_path, reg_time)) = found_output {
            let time_str = if reg_time > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let elapsed = std::time::Duration::from_secs((now - reg_time).max(0) as u64);
                format_duration_ago(elapsed)
            } else {
                "unknown".to_string()
            };
            (BuildStatus::Passed, Some(out_path), true, time_str)
        } else {
            // No output — check if a build log exists (failed build)
            let drv_basename = old_drv_path.split('/').next_back().unwrap_or("");
            let log_exists = if drv_basename.len() >= 3 {
                let prefix = &drv_basename[..2];
                let rest = &drv_basename[2..];
                let log_file = format!("/nix/var/log/nix/drvs/{}/{}.bz2", prefix, rest);
                std::path::Path::new(&log_file).exists()
            } else {
                false
            };
            let time_str = std::fs::metadata(old_drv_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|mtime| {
                    let elapsed = mtime.elapsed().unwrap_or_default();
                    format_duration_ago(elapsed)
                })
                .unwrap_or_else(|| "unknown".to_string());
            if log_exists {
                (BuildStatus::Failed, None, false, time_str)
            } else {
                (BuildStatus::Unknown, None, false, time_str)
            }
        };

        // Retrieve build log
        let log_lines = try_cached_log(nix, old_drv_path, "historical build")
            .or_else(|| try_direct_log_file(old_drv_path))
            .unwrap_or_default();

        let short_hash = &drv_hash[..std::cmp::min(7, drv_hash.len())];

        // Emit a Build for each flake attribute that maps to this drv name
        for (flake_ref, derivation_attr) in &attrs {
            historical_builds.push(Build {
                id: 0, // Will be reassigned later
                derivation: derivation_attr.to_string(),
                status: status.clone(),
                duration: "—".to_string(),
                time: time_str.clone(),
                branch: Some("historical".to_string()),
                commit: format!("{}…{}", short_hash, drv_name),
                owner: None,
                repo: None,
                forge_url: None,
                flake_ref: flake_ref.to_string(),
                pr: None,
                override_inputs: vec![],
                log: log_lines.clone(),
                drv_path: Some(old_drv_path.clone()),
                store_path: store_path.clone(),
                in_store,
                historical: true,
                dirty: false,
                is_alias: false,
            });
        }
    }

    // Sort historical builds by time (most recent first based on drv hash — approximation)
    historical_builds.sort_by(|a, b| a.commit.cmp(&b.commit));

    historical_builds
}

/// Resolve historical builds to their real git commits by matching derivation hashes.
///
/// For each commit in git history, evaluates `nix path-info --derivation` to get
/// the exact `.drv` path, then compares against historical builds' drv hashes.
/// When matched, updates the historical build with the real git commit, forge URL,
/// owner, and repo.
fn resolve_historical_commits(nix: &str, builds: &mut [Build], metadata: &FlakeMetadata) {
    use std::collections::{HashMap, HashSet};

    // Collect historical builds' drv hashes → build indices
    let mut drv_hash_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
    let mut historical_attrs: HashSet<String> = HashSet::new();

    for (i, build) in builds.iter().enumerate() {
        if !build.historical {
            continue;
        }
        let drv_path = match &build.drv_path {
            Some(p) => p,
            None => continue,
        };
        let drv_filename = drv_path.split('/').next_back().unwrap_or("");
        if drv_filename.len() <= 33 || drv_filename.as_bytes()[32] != b'-' {
            continue;
        }
        let hash = &drv_filename[..32];
        drv_hash_to_indices
            .entry(hash.to_string())
            .or_default()
            .push(i);
        historical_attrs.insert(build.derivation.clone());
    }

    if drv_hash_to_indices.is_empty() {
        return;
    }

    // Determine git directory from the first build's flake_ref
    let local_dir = builds
        .iter()
        .find(|b| !b.historical)
        .and_then(|b| git_dir_from_flake_ref(&b.flake_ref));

    // Resolve forge_url/owner/repo — from local git or flake metadata
    let (forge_url, owner, repo) = if let Some(ref dir) = local_dir {
        git_remote_info(dir)
    } else {
        // Remote ref: derive from src input or flake URL
        if let Some(src) = metadata.inputs.iter().find(|i| i.name == "src") {
            parse_git_url(&src.url)
        } else {
            parse_git_url(&metadata.resolved_url)
        }
    };

    // For remote refs without local git history, we can't walk git log to match
    // drv hashes to commits. Populate forge info on historical builds and return.
    let dir = match local_dir {
        Some(d) => d,
        None => {
            tracing::info!(
                historical_drvs = drv_hash_to_indices.len(),
                "Remote flake ref — cannot resolve historical builds to git commits (no local git history)"
            );
            // Still populate forge_url/owner/repo so dashboard links work
            for indices in drv_hash_to_indices.values() {
                for &idx in indices {
                    builds[idx].forge_url = forge_url.clone();
                    builds[idx].owner = owner.clone();
                    builds[idx].repo = repo.clone();
                }
            }
            return;
        }
    };

    // Verify it's a git repo
    if git_commit(&dir).is_none() {
        return;
    }

    // Get unique attribute paths from historical builds
    let attrs: Vec<String> = historical_attrs.into_iter().collect();

    // Get recent commits
    let (ok, stdout, _) = run_cmd_full("git", &["-C", &dir, "log", "--format=%H", "-200"]);
    if !ok {
        return;
    }
    let commits: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();

    // Track which attributes still have unresolved historical builds
    let mut unresolved_attrs: HashSet<String> = attrs.iter().cloned().collect();
    let mut unresolved_hashes: HashSet<String> = drv_hash_to_indices.keys().cloned().collect();

    // Track which (attr) has been fully resolved
    let mut attr_resolved_count: HashMap<String, usize> = HashMap::new();
    let mut attr_total_count: HashMap<String, usize> = HashMap::new();

    for (hash, indices) in &drv_hash_to_indices {
        for &idx in indices {
            let attr = &builds[idx].derivation;
            *attr_total_count.entry(attr.clone()).or_default() += 1;
            // Don't initialize resolved count — starts at 0
            let _ = hash; // suppress unused warning
        }
    }

    tracing::info!(
        historical_drvs = drv_hash_to_indices.len(),
        attrs = attrs.len(),
        commits = commits.len(),
        "Resolving historical builds to git commits"
    );

    for commit in &commits {
        if unresolved_hashes.is_empty() {
            break;
        }

        for attr in &attrs {
            // Skip attributes that are fully resolved
            if !unresolved_attrs.contains(attr) {
                continue;
            }

            let target = format!("git+file:{}?rev={}#{}", dir, commit, attr);
            let (ok, stdout, _) = run_cmd_full(nix, &["path-info", "--derivation", &target]);

            let drv_path = stdout.trim().to_string();
            if !ok && !drv_path.starts_with("/nix/store/") {
                continue;
            }

            // Extract the drv hash from the result
            let drv_filename = drv_path.split('/').next_back().unwrap_or("");
            if drv_filename.len() <= 33 || drv_filename.as_bytes()[32] != b'-' {
                continue;
            }
            let result_hash = &drv_filename[..32];

            if let Some(indices) = drv_hash_to_indices.get(result_hash) {
                if unresolved_hashes.contains(result_hash) {
                    tracing::info!(
                        commit = &commit[..8],
                        attr,
                        drv_hash = &result_hash[..8],
                        "Matched historical build to git commit"
                    );

                    for &idx in indices {
                        builds[idx].commit = commit.to_string();
                        builds[idx].forge_url = forge_url.clone();
                        builds[idx].owner = owner.clone();
                        builds[idx].repo = repo.clone();
                    }

                    unresolved_hashes.remove(result_hash);

                    // Track resolution progress per attribute
                    let resolved = attr_resolved_count.entry(attr.clone()).or_default();
                    *resolved += indices.len();
                    if let Some(&total) = attr_total_count.get(attr) {
                        if *resolved >= total {
                            unresolved_attrs.remove(attr);
                        }
                    }
                }
            }
        }
    }

    if !unresolved_hashes.is_empty() {
        tracing::info!(
            remaining = unresolved_hashes.len(),
            "Some historical builds could not be matched to git commits"
        );
    }
}

/// Format a duration as a human-readable "ago" string
fn format_duration_ago(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86400 {
        let hours = secs / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = secs / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}

/// Discover builds from the nix store using attribute name hints when flake
/// evaluation fails (all drv_paths are None).
///
/// Extracts leaf names from attribute paths (e.g. `packages.x86_64-linux.susui` → `susui`),
/// queries the nix DB for matching `.drv` paths, checks outputs, and creates Build records.
fn discover_builds_by_attr_hints(
    nix: &str,
    failed_attrs: &[String],
    flake_ref: &str,
    git_info: &ResolvedGitInfo,
    exclude_hashes: &std::collections::HashSet<String>,
) -> Vec<Build> {
    use std::collections::HashMap;

    // Extract leaf names from attribute paths, skipping "default"
    // Also track which category each attr belongs to for alias detection
    let mut leaf_names: Vec<String> = Vec::new();
    let mut default_attrs: Vec<String> = Vec::new();
    let mut leaf_to_attrs: HashMap<String, Vec<String>> = HashMap::new();

    for attr in failed_attrs {
        let leaf = attr.split('.').next_back().unwrap_or(attr);
        if leaf == "default" {
            default_attrs.push(attr.clone());
        } else {
            if !leaf_names.contains(&leaf.to_string()) {
                leaf_names.push(leaf.to_string());
            }
            leaf_to_attrs
                .entry(leaf.to_string())
                .or_default()
                .push(attr.clone());
        }
    }

    if leaf_names.is_empty() {
        return Vec::new();
    }

    // Query the nix DB for .drv paths matching the leaf names as prefixes
    // e.g. "susui" matches "susui-0.1.0"
    let db_matches = find_drvs_by_leaf_prefix(&leaf_names);

    if db_matches.is_empty() {
        tracing::debug!("No matching .drv paths found in nix DB for attr hints");
        return Vec::new();
    }

    // Filter out excluded hashes
    let filtered: Vec<(String, String, String)> = db_matches
        .into_iter()
        .filter(|(_path, _name, hash)| !exclude_hashes.contains(hash))
        .collect();

    if filtered.is_empty() {
        return Vec::new();
    }

    // Group by derivation name and pick the best (most recent) for each leaf
    // drv_name → Vec<(drv_path, drv_name, drv_hash)>
    let mut by_name: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for entry in &filtered {
        by_name.entry(entry.1.clone()).or_default().push(entry.clone());
    }

    // Map each leaf name to its matching drv name group
    // "susui" matches "susui-0.1.0" if the drv name starts with "susui-" or equals "susui"
    let mut leaf_matches: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for (drv_name, entries) in &by_name {
        for leaf in &leaf_names {
            if drv_name == leaf || drv_name.starts_with(&format!("{}-", leaf)) {
                leaf_matches
                    .entry(leaf.clone())
                    .or_default()
                    .extend(entries.clone());
            }
        }
    }

    // Query outputs for all matched drvs
    let all_drv_paths: Vec<String> = filtered.iter().map(|(p, _, _)| p.clone()).collect();
    let outputs_map = crate::nixdb::find_outputs_for_drvs(&all_drv_paths);

    let mut builds = Vec::new();

    // For each leaf name match, create Build records
    for (leaf, matches) in &leaf_matches {
        let attrs = match leaf_to_attrs.get(leaf) {
            Some(a) => a,
            None => continue,
        };

        for (drv_path, _drv_name, _drv_hash) in matches {
            // Check outputs
            let db_outputs = outputs_map.get(drv_path.as_str());
            let found_output = db_outputs.and_then(|outs| {
                outs.iter()
                    .find(|(path, _)| std::path::Path::new(path).exists())
                    .cloned()
            });

            let (status, store_path, in_store, time_str) = if let Some((out_path, reg_time)) = found_output {
                let time_str = if reg_time > 0 {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    let elapsed = std::time::Duration::from_secs((now - reg_time).max(0) as u64);
                    format_duration_ago(elapsed)
                } else {
                    "unknown".to_string()
                };
                (BuildStatus::Passed, Some(out_path), true, time_str)
            } else {
                // Check for build log
                let drv_basename = drv_path.split('/').next_back().unwrap_or("");
                let log_exists = if drv_basename.len() >= 3 {
                    let prefix = &drv_basename[..2];
                    let rest = &drv_basename[2..];
                    let log_file = format!("/nix/var/log/nix/drvs/{}/{}.bz2", prefix, rest);
                    std::path::Path::new(&log_file).exists()
                } else {
                    false
                };
                let time_str = std::fs::metadata(drv_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|mtime| format_duration_ago(mtime.elapsed().unwrap_or_default()))
                    .unwrap_or_else(|| "unknown".to_string());
                if log_exists {
                    (BuildStatus::Failed, None, false, time_str)
                } else {
                    (BuildStatus::Unknown, None, false, time_str)
                }
            };

            let log_lines = try_cached_log(nix, drv_path, "discovered build")
                .or_else(|| try_direct_log_file(drv_path))
                .unwrap_or_default();

            for attr in attrs {
                builds.push(Build {
                    id: 0,
                    derivation: attr.clone(),
                    status: status.clone(),
                    duration: "—".to_string(),
                    time: time_str.clone(),
                    branch: git_info.branch.clone(),
                    commit: git_info.commit.clone(),
                    owner: git_info.owner.clone(),
                    repo: git_info.repo.clone(),
                    forge_url: git_info.forge_url.clone(),
                    flake_ref: flake_ref.to_string(),
                    pr: None,
                    override_inputs: vec![],
                    log: log_lines.clone(),
                    drv_path: Some(drv_path.clone()),
                    store_path: store_path.clone(),
                    in_store,
                    historical: true,
                    dirty: false,
                    is_alias: false,
                });
            }
        }
    }

    // Handle default aliases: if packages.x86_64-linux.default exists and
    // there's exactly one packages.* match, emit a Build for default too
    for default_attr in &default_attrs {
        // Find the category prefix: "packages.x86_64-linux.default" → "packages.x86_64-linux."
        let prefix = default_attr.rsplit_once('.').map(|(p, _)| format!("{}.", p)).unwrap_or_default();
        if prefix.is_empty() {
            continue;
        }

        // Find builds matching this category prefix
        let category_builds: Vec<&Build> = builds
            .iter()
            .filter(|b| b.derivation.starts_with(&prefix) && b.derivation != *default_attr)
            .collect();

        if category_builds.len() == 1 {
            // Exactly one match — create an alias
            let source = category_builds[0];
            builds.push(Build {
                id: 0,
                derivation: default_attr.clone(),
                status: source.status.clone(),
                duration: source.duration.clone(),
                time: source.time.clone(),
                branch: source.branch.clone(),
                commit: source.commit.clone(),
                owner: source.owner.clone(),
                repo: source.repo.clone(),
                forge_url: source.forge_url.clone(),
                flake_ref: source.flake_ref.clone(),
                pr: None,
                override_inputs: vec![],
                log: source.log.clone(),
                drv_path: source.drv_path.clone(),
                store_path: source.store_path.clone(),
                in_store: source.in_store,
                historical: true,
                dirty: false,
                is_alias: true,
            });
        }
    }

    // Re-assign IDs
    for (i, build) in builds.iter_mut().enumerate() {
        build.id = (i + 1) as u64;
    }

    builds
}

/// Query the nix DB for .drv paths where the derivation name starts with one
/// of the given leaf prefixes (e.g. "susui" matches "susui-0.1.0").
fn find_drvs_by_leaf_prefix(leaf_names: &[String]) -> Vec<(String, String, String)> {
    use rusqlite::{Connection, OpenFlags};

    let conn = match Connection::open_with_flags(
        "/nix/var/nix/db/db.sqlite",
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("nixdb: failed to open for leaf prefix query: {}", e);
            return Vec::new();
        }
    };

    let mut results = Vec::new();

    for leaf in leaf_names {
        // Match exact name or name with version suffix: "susui" → "%-susui.drv" or "%-susui-%.drv"
        for like_pattern in &[
            format!("/nix/store/%-{}.drv", leaf),
            format!("/nix/store/%-{}-%", leaf),  // versioned: susui-0.1.0.drv
        ] {
            let mut stmt = match conn.prepare(
                "SELECT path FROM ValidPaths WHERE path LIKE ? AND path LIKE '%.drv'",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rows = match stmt.query_map([like_pattern], |row| row.get::<_, String>(0)) {
                Ok(r) => r,
                Err(_) => continue,
            };

            for path in rows.flatten() {
                let fname = path.split('/').next_back().unwrap_or("");
                if fname.len() > 33 && fname.as_bytes()[32] == b'-' {
                    let hash = fname[..32].to_string();
                    let name = fname[33..].trim_end_matches(".drv").to_string();
                    // Verify the name actually starts with this leaf
                    if name == *leaf || name.starts_with(&format!("{}-", leaf)) {
                        results.push((path, name, hash));
                    }
                }
            }
        }
    }

    // Deduplicate by drv_path
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.dedup_by(|a, b| a.0 == b.0);

    results
}

/// Scan a flake and return builds for all discovered outputs (evaluation only, no builds triggered)
fn scan_flake(flake_ref: &str, metadata: &FlakeMetadata, overrides: &[(String, String)]) -> Result<Vec<Build>> {
    let nix = nix_bin();
    tracing::info!(flake_ref, "Scanning flake outputs");

    let git_info = resolve_git_info(flake_ref, metadata);

    let outputs = list_flake_outputs(&nix, flake_ref, overrides)
        .with_context(|| format!("Failed to list outputs for {}", flake_ref))?;

    tracing::info!(count = outputs.len(), "Found flake outputs");

    let mut builds = Vec::new();
    for (i, attr) in outputs.iter().enumerate() {
        tracing::info!(attr, "Processing derivation");
        let build = eval_derivation(flake_ref, attr, overrides, (i + 1) as u64, &git_info);
        builds.push(build);
    }

    // Check if all evaluations failed (all drv_paths are None)
    let all_failed = !builds.is_empty() && builds.iter().all(|b| b.drv_path.is_none());

    if all_failed {
        // Use attribute name hints to discover builds from the store
        let failed_attrs: Vec<String> = outputs.clone();
        let discovered = discover_builds_by_attr_hints(
            &nix,
            &failed_attrs,
            flake_ref,
            &git_info,
            &std::collections::HashSet::new(),
        );
        if !discovered.is_empty() {
            tracing::info!(
                count = discovered.len(),
                "Discovered builds from store (eval failed, using attr hints)"
            );
            // Replace failed builds with discovered builds
            builds = discovered;
            // Re-assign IDs
            for (i, build) in builds.iter_mut().enumerate() {
                build.id = (i + 1) as u64;
            }
        }
        resolve_historical_commits(&nix, &mut builds, metadata);
    } else {
        // Normal path: detect aliases, sort, find historical, resolve commits
        detect_aliases(&mut builds);
        sort_by_dependency_order(&mut builds);

        // Discover historical builds from the nix DB
        let historical = find_historical_builds(&nix, &builds);
        if !historical.is_empty() {
            tracing::info!(count = historical.len(), "Found historical builds in store");
            let base_id = builds.len() as u64;
            for (i, mut hbuild) in historical.into_iter().enumerate() {
                hbuild.id = base_id + (i as u64) + 1;
                builds.push(hbuild);
            }
            resolve_historical_commits(&nix, &mut builds, metadata);
        }
    }

    enrich_build_durations(&mut builds);

    Ok(builds)
}

/// Detect alias builds in the normal evaluation path.
///
/// When `packages.default` and `packages.susui` share the same `drv_path`,
/// mark `default` as an alias.
fn detect_aliases(builds: &mut [Build]) {
    use std::collections::HashMap;

    // Build map of drv_path → list of indices
    let mut drv_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, build) in builds.iter().enumerate() {
        if let Some(ref drv) = build.drv_path {
            drv_to_indices.entry(drv.clone()).or_default().push(i);
        }
    }

    // For groups sharing the same drv_path, mark "default" entries as aliases
    for indices in drv_to_indices.values() {
        if indices.len() < 2 {
            continue;
        }
        let has_non_default = indices.iter().any(|&i| {
            builds[i]
                .derivation
                .split('.')
                .next_back()
                .unwrap_or("")
                != "default"
        });
        if !has_non_default {
            continue;
        }
        for &i in indices {
            let leaf = builds[i].derivation.split('.').next_back().unwrap_or("");
            if leaf == "default" {
                builds[i].is_alias = true;
            }
        }
    }
}

/// Enrich builds with real durations from the Nix SQLite database.
///
/// Computes `output.registrationTime - deriver.registrationTime` which gives
/// the actual build duration for locally-built paths. Substituted paths yield
/// 0 and are left unchanged (showing "—" or eval time).
fn enrich_build_durations(builds: &mut [Build]) {
    let store_paths: Vec<&str> = builds
        .iter()
        .filter_map(|b| b.store_path.as_deref())
        .collect();
    let drv_paths: Vec<&str> = builds
        .iter()
        .filter_map(|b| b.drv_path.as_deref())
        .collect();

    if store_paths.is_empty() && drv_paths.is_empty() {
        return;
    }

    let durations = crate::nixdb::lookup_build_durations(&store_paths, &drv_paths);
    if durations.by_store_path.is_empty() && durations.by_drv_path.is_empty() {
        return;
    }

    for build in builds.iter_mut() {
        // Try matching by store_path first, then fall back to drv_path
        let secs: Option<u64> = build
            .store_path
            .as_deref()
            .and_then(|sp| durations.by_store_path.get(sp).copied())
            .or_else(|| {
                build
                    .drv_path
                    .as_deref()
                    .and_then(|dp| durations.by_drv_path.get(dp).copied())
            });

        if let Some(secs) = secs {
            build.duration = format_duration(std::time::Duration::from_secs(secs));
        }
    }
}

/// Sort builds by dependency order to match continuous build log output.
///
/// The ordering principle: package builds appear first, followed by their
/// associated checks (which share the same `drv_path`), then independent
/// checks, and finally dev shells.  Within each group, builds sharing the
/// same derivation path are kept adjacent so the dashboard reads like a
/// continuous build log where tests run right after the build they verify.
fn sort_by_dependency_order(builds: &mut [Build]) {
    use std::collections::HashSet;

    // Collect drvPaths that belong to package outputs (owned to avoid borrow conflict)
    let package_drvs: HashSet<String> = builds
        .iter()
        .filter(|b| b.derivation.starts_with("packages."))
        .filter_map(|b| b.drv_path.clone())
        .collect();

    // Sort key: (category, drv_path, is_not_package, derivation)
    //
    // category:
    //   0 = packages and checks whose drvPath matches a package (same build)
    //   1 = independent checks (own drvPath, no matching package)
    //   2 = devShells
    //   3 = everything else
    //
    // Grouping by drv_path keeps same-derivation builds adjacent.
    // is_not_package ensures packages sort before their matching checks.
    builds.sort_by(|a, b| {
        let key = |build: &Build| -> (u8, String, bool, String) {
            let drv = build.drv_path.as_deref().unwrap_or("");
            let is_pkg = build.derivation.starts_with("packages.");
            let is_check = build.derivation.starts_with("checks.");
            let is_shell = build.derivation.starts_with("devShells.");

            let category = if is_pkg {
                0
            } else if is_check {
                if !drv.is_empty() && package_drvs.contains(drv) {
                    0 // group with its package
                } else {
                    1
                }
            } else if is_shell {
                2
            } else {
                3
            };

            (category, drv.to_string(), !is_pkg, build.derivation.clone())
        };
        key(a).cmp(&key(b))
    });

    // Re-assign sequential IDs to match the new order
    for (i, build) in builds.iter_mut().enumerate() {
        build.id = (i + 1) as u64;
    }
}

/// Append `--override-input NAME URI` arguments to a command argument list.
fn append_override_args<'a>(args: &mut Vec<&'a str>, overrides: &'a [(String, String)]) {
    for (name, uri) in overrides {
        args.push("--override-input");
        args.push(name.as_str());
        args.push(uri.as_str());
    }
}

/// Collect all data for a flake: metadata + builds (evaluation only, no builds triggered)
pub fn collect_all(flake_ref: &str, overrides: &[(String, String)]) -> Result<(FlakeMetadata, Vec<Build>)> {
    let metadata = collect_flake_metadata(flake_ref, overrides)?;
    let builds = scan_flake(flake_ref, &metadata, overrides)?;
    Ok((metadata, builds))
}

// ─── Helpers ──────────────────────────────────────────────

/// Lines from nix stderr that are infrastructure noise, not build output.
/// These get emitted by the nix daemon/store layer and have nothing to do
/// with the derivation being evaluated or built.
fn is_nix_noise(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() {
        return true;
    }
    // Daemon/store infrastructure warnings
    if l.contains("the group 'nixbld' specified in 'build-users-group' does not exist") {
        return true;
    }
    if l.starts_with("error (ignored):") && l.contains("opening file '/nix/store/") {
        return true;
    }
    // Flake lock chatter
    if l.starts_with("warning: updating lock file") || l.starts_with("warning: not writing modified lock file") {
        return true;
    }
    // Git fetch noise
    if l.starts_with("unpacking '") && l.contains("into the Git cache") {
        return true;
    }
    // Nix structured log markers (internal JSON annotations)
    if l.starts_with("@nix ") || l.starts_with("@nix\t") {
        return true;
    }
    // `nix log` preamble
    if l.starts_with("got build log for '") {
        return true;
    }
    false
}

/// Build log lines from raw output, filtering out nix infrastructure noise
fn make_log_lines(output: &str) -> Vec<LogLine> {
    output
        .lines()
        .filter(|line| !is_nix_noise(line))
        .enumerate()
        .map(|(i, line)| LogLine {
            n: i + 1,
            text: line.to_string(),
            level: classify_log_line(line),
        })
        .collect()
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

fn classify_log_line(line: &str) -> String {
    let l = line.to_lowercase();
    // Test results
    if l.contains("test result:") {
        if l.contains("0 failed") {
            return "success".to_string();
        } else {
            return "error".to_string();
        }
    }
    if l.contains("... ok") || l.contains("... bench:") {
        return "success".to_string();
    }
    if l.contains("... failed") || l.contains("... ignored") {
        return "error".to_string();
    }
    if l.starts_with("running ") && l.contains("test") {
        return "info".to_string();
    }
    // Phase headers
    if l.starts_with("running phase:") || l.ends_with("phase") {
        return "nix".to_string();
    }
    if l.contains("completed in") {
        return "dim".to_string();
    }
    // Errors
    if l.contains("error") || l.contains("failed") || l.contains("fail:") {
        return "error".to_string();
    }
    // Warnings
    if l.contains("warning") || l.contains("override-input") {
        return "warning".to_string();
    }
    // Success
    if l.contains("success") || l.contains("built successfully") || l.starts_with("/nix/store/") {
        return "success".to_string();
    }
    // Cargo output
    if l.trim_start().starts_with("compiling ") || l.trim_start().starts_with("downloading ") {
        return "dim".to_string();
    }
    if l.trim_start().starts_with("finished ") {
        return "success".to_string();
    }
    // Nix build phases
    if l.contains("evaluating") || l.contains("copying") || l.starts_with("building") {
        return "nix".to_string();
    }
    // Hook execution
    if l.starts_with("executing ") || l.starts_with("finished ") {
        return "dim".to_string();
    }
    if l.starts_with("  ") || l.contains("...") {
        return "dim".to_string();
    }
    "info".to_string()
}

fn parse_flake_uri(uri: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    if let Some(rest) = uri.strip_prefix("github:") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        match parts.len() {
            3 => (
                "github".into(),
                Some(parts[0].into()),
                Some(parts[1].into()),
                Some(parts[2].into()),
            ),
            2 => (
                "github".into(),
                Some(parts[0].into()),
                Some(parts[1].into()),
                None,
            ),
            _ => ("github".into(), None, None, None),
        }
    } else if uri.starts_with("path:") || uri.starts_with("./") || uri.starts_with('/') {
        ("path".into(), None, None, Some(uri.into()))
    } else {
        ("indirect".into(), None, None, Some(uri.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(std::time::Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(std::time::Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn test_classify_log_line() {
        assert_eq!(classify_log_line("error: build failed"), "error");
        assert_eq!(classify_log_line("  override-input: foo"), "warning");
        assert_eq!(classify_log_line("/nix/store/abc-pkg"), "success");
        assert_eq!(classify_log_line("evaluating derivation"), "nix");
        assert_eq!(classify_log_line("  configuring..."), "dim");
        assert_eq!(classify_log_line("something else"), "info");
    }

    #[test]
    fn test_parse_flake_uri() {
        let (t, o, r, rf) = parse_flake_uri("github:NixOS/nixpkgs/main");
        assert_eq!(t, "github");
        assert_eq!(o.unwrap(), "NixOS");
        assert_eq!(r.unwrap(), "nixpkgs");
        assert_eq!(rf.unwrap(), "main");
    }

    #[test]
    fn test_is_nix_noise() {
        assert!(is_nix_noise("warning: the group 'nixbld' specified in 'build-users-group' does not exist"));
        assert!(is_nix_noise("error (ignored): opening file '/nix/store/rbfgknm995x1rwpnmsn1d0c792r257hz-stdenv-linux.drv': No such file or directory"));
        assert!(is_nix_noise("unpacking 'github:NixOS/nixpkgs/abc123' into the Git cache..."));
        assert!(is_nix_noise(""));
        assert!(is_nix_noise("   "));
        // Real output should NOT be filtered
        assert!(!is_nix_noise("error: build failed"));
        assert!(!is_nix_noise("/nix/store/abc-susui-0.1.0.drv"));
        assert!(!is_nix_noise("building '/nix/store/abc.drv'..."));
    }

    #[test]
    fn test_make_log_lines_filters_noise() {
        let output = "\
warning: the group 'nixbld' specified in 'build-users-group' does not exist
/nix/store/kgx2sg81s17x59a7h0ng5mzvj4v1rqm3-susui-0.1.0.drv
error (ignored): opening file '/nix/store/xxx-stdenv.drv': No such file or directory
";
        let lines = make_log_lines(output);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.contains("susui-0.1.0.drv"));
        assert_eq!(lines[0].n, 1);
    }

    #[test]
    fn test_build_stats() {
        let builds = vec![
            Build {
                id: 1, derivation: "a".into(), status: BuildStatus::Passed,
                duration: "1s".into(), time: "now".into(), branch: None,
                commit: "abc".into(), owner: None, repo: None, forge_url: None,
                flake_ref: ".".into(), pr: None, override_inputs: vec![], log: vec![],
                drv_path: None, store_path: None, in_store: true, historical: false, dirty: false, is_alias: false,
            },
            Build {
                id: 2, derivation: "b".into(), status: BuildStatus::Failed,
                duration: "2s".into(), time: "now".into(), branch: None,
                commit: "def".into(), owner: None, repo: None, forge_url: None,
                flake_ref: ".".into(), pr: None,
                override_inputs: vec![OverrideInput {
                    input_name: "nixpkgs".into(), input_type: "github".into(),
                    owner: Some("NixOS".into()), repo: Some("nixpkgs".into()),
                    git_ref: Some("main".into()), pr: None,
                }],
                log: vec![],
                drv_path: None, store_path: None, in_store: false, historical: false, dirty: false, is_alias: false,
            },
        ];
        let stats = BuildStats::from_builds(&builds);
        assert_eq!(stats.all, 2);
        assert_eq!(stats.passed, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.overridden, 1);
        assert_eq!(stats.in_store, 1);
        assert!((stats.success_rate - 50.0).abs() < 0.01);
    }

    fn make_build(id: u64, derivation: &str, drv_path: Option<&str>) -> Build {
        Build {
            id,
            derivation: derivation.into(),
            status: BuildStatus::Passed,
            duration: "1s".into(),
            time: "now".into(),
            branch: None,
            commit: "abc".into(),
            owner: None,
            repo: None,
            forge_url: None,
            flake_ref: ".".into(),
            pr: None,
            override_inputs: vec![],
            log: vec![],
            drv_path: drv_path.map(|s| s.to_string()),
            store_path: None,
            in_store: false,
            historical: false,
            dirty: false,
            is_alias: false,
        }
    }

    #[test]
    fn test_sort_by_dependency_order() {
        // Alphabetical input (as nix flake show would produce)
        let mut builds = vec![
            make_build(1, "checks.x86_64-linux.clippy", Some("/nix/store/aaa-clippy.drv")),
            make_build(2, "checks.x86_64-linux.susui", Some("/nix/store/bbb-susui.drv")),
            make_build(3, "devShells.x86_64-linux.default", Some("/nix/store/ccc-shell.drv")),
            make_build(4, "packages.x86_64-linux.default", Some("/nix/store/bbb-susui.drv")),
            make_build(5, "packages.x86_64-linux.susui", Some("/nix/store/bbb-susui.drv")),
        ];

        sort_by_dependency_order(&mut builds);

        let order: Vec<&str> = builds.iter().map(|b| b.derivation.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "packages.x86_64-linux.default",   // package build first
                "packages.x86_64-linux.susui",      // same drv, also a package
                "checks.x86_64-linux.susui",        // same drv as packages, tests after build
                "checks.x86_64-linux.clippy",       // independent check
                "devShells.x86_64-linux.default",   // dev shell last
            ]
        );

        // IDs should be re-assigned sequentially
        let ids: Vec<u64> = builds.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_sort_preserves_order_for_unrelated_checks() {
        // Multiple independent checks should remain in stable (alphabetical) order
        let mut builds = vec![
            make_build(1, "checks.x86_64-linux.audit", Some("/nix/store/aaa.drv")),
            make_build(2, "checks.x86_64-linux.clippy", Some("/nix/store/bbb.drv")),
            make_build(3, "checks.x86_64-linux.fmt", Some("/nix/store/ccc.drv")),
            make_build(4, "packages.x86_64-linux.default", Some("/nix/store/ddd.drv")),
        ];

        sort_by_dependency_order(&mut builds);

        let order: Vec<&str> = builds.iter().map(|b| b.derivation.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "packages.x86_64-linux.default",
                "checks.x86_64-linux.audit",
                "checks.x86_64-linux.clippy",
                "checks.x86_64-linux.fmt",
            ]
        );
    }

    #[test]
    fn test_git_dir_from_flake_ref() {
        assert_eq!(git_dir_from_flake_ref("."), Some(".".into()));
        assert_eq!(git_dir_from_flake_ref("./my/proj"), Some("./my/proj".into()));
        assert_eq!(git_dir_from_flake_ref("/abs/path"), Some("/abs/path".into()));
        assert_eq!(git_dir_from_flake_ref("path:/foo"), Some("/foo".into()));
        assert_eq!(git_dir_from_flake_ref("path:./rel"), Some("./rel".into()));
        assert_eq!(git_dir_from_flake_ref("git+file:///home/user/proj"), Some("/home/user/proj".into()));
        assert_eq!(git_dir_from_flake_ref("git+file:./rel"), Some("./rel".into()));
        assert_eq!(git_dir_from_flake_ref("github:NixOS/nixpkgs"), None);
        assert_eq!(git_dir_from_flake_ref("git+ssh://example.com/repo"), None);
        assert_eq!(git_dir_from_flake_ref("nixpkgs"), None);
    }

    #[test]
    fn test_parse_git_url_ssh() {
        // Plain ssh://
        let (forge, owner, repo) = parse_git_url("ssh://git@github.ibm.com/GSKit/GSKit8-repo.git");
        assert_eq!(forge.unwrap(), "https://github.ibm.com");
        assert_eq!(owner.unwrap(), "GSKit");
        assert_eq!(repo.unwrap(), "GSKit8-repo");
    }

    #[test]
    fn test_parse_git_url_git_plus_ssh() {
        // Nix-style git+ssh:// — should strip git+ prefix
        let (forge, owner, repo) = parse_git_url("git+ssh://git@github.ibm.com/GSKit/GSKit8-repo");
        assert_eq!(forge.unwrap(), "https://github.ibm.com");
        assert_eq!(owner.unwrap(), "GSKit");
        assert_eq!(repo.unwrap(), "GSKit8-repo");
    }

    #[test]
    fn test_parse_git_url_git_plus_https() {
        let (forge, owner, repo) = parse_git_url("git+https://github.com/NixOS/nixpkgs.git");
        assert_eq!(forge.unwrap(), "https://github.com");
        assert_eq!(owner.unwrap(), "NixOS");
        assert_eq!(repo.unwrap(), "nixpkgs");
    }

    #[test]
    fn test_parse_git_url_with_query_params() {
        let (forge, owner, repo) = parse_git_url("git+ssh://git@github.ibm.com/GSKit/GSKit8-repo?rev=abc123&ref=main");
        assert_eq!(forge.unwrap(), "https://github.ibm.com");
        assert_eq!(owner.unwrap(), "GSKit");
        assert_eq!(repo.unwrap(), "GSKit8-repo");
    }

    #[test]
    fn test_parse_git_url_with_fragment() {
        let (forge, owner, repo) = parse_git_url("git+ssh://git@github.ibm.com/GSKit/GSKit8-repo#packages.x86_64-linux.default");
        assert_eq!(forge.unwrap(), "https://github.ibm.com");
        assert_eq!(owner.unwrap(), "GSKit");
        assert_eq!(repo.unwrap(), "GSKit8-repo");
    }

    #[test]
    fn test_parse_git_url_with_query_and_fragment() {
        let (forge, owner, repo) = parse_git_url("git+ssh://git@example.com/owner/repo?ref=dev#attr");
        assert_eq!(forge.unwrap(), "https://example.com");
        assert_eq!(owner.unwrap(), "owner");
        assert_eq!(repo.unwrap(), "repo");
    }

    #[test]
    fn test_parse_git_url_github_shorthand() {
        let (forge, owner, repo) = parse_git_url("github:NixOS/nixpkgs");
        assert_eq!(forge.unwrap(), "https://github.com");
        assert_eq!(owner.unwrap(), "NixOS");
        assert_eq!(repo.unwrap(), "nixpkgs");
    }

    #[test]
    fn test_parse_git_url_git_at() {
        let (forge, owner, repo) = parse_git_url("git@gitlab.com:myorg/myrepo.git");
        assert_eq!(forge.unwrap(), "https://gitlab.com");
        assert_eq!(owner.unwrap(), "myorg");
        assert_eq!(repo.unwrap(), "myrepo");
    }

    #[test]
    fn test_parse_git_url_https() {
        let (forge, owner, repo) = parse_git_url("https://github.com/owner/repo.git");
        assert_eq!(forge.unwrap(), "https://github.com");
        assert_eq!(owner.unwrap(), "owner");
        assert_eq!(repo.unwrap(), "repo");
    }

    #[test]
    fn test_parse_git_url_unknown() {
        let (forge, owner, repo) = parse_git_url("nixpkgs");
        assert!(forge.is_none());
        assert!(owner.is_none());
        assert!(repo.is_none());
    }

    #[test]
    fn test_resolve_git_info_src_input_fallback() {
        // When flake_ref is remote (no local git dir), should fall back to src input
        let metadata = FlakeMetadata {
            description: None,
            url: "git+ssh://git@github.ibm.com/mattgreen/gskpkgs".to_string(),
            resolved_url: "git+ssh://git@github.ibm.com/mattgreen/gskpkgs".to_string(),
            revision: Some("aaa111".to_string()),
            inputs: vec![
                FlakeInput {
                    name: "src".to_string(),
                    input_type: "git".to_string(),
                    url: "git+ssh://git@github.ibm.com/GSKit/GSKit8-repo".to_string(),
                    locked_rev: Some("bbb222".to_string()),
                    locked_ref: Some("main".to_string()),
                    last_modified: None,
                },
                FlakeInput {
                    name: "nixpkgs".to_string(),
                    input_type: "github".to_string(),
                    url: "github:NixOS/nixpkgs".to_string(),
                    locked_rev: Some("ccc333".to_string()),
                    locked_ref: None,
                    last_modified: None,
                },
            ],
        };

        let info = resolve_git_info("git+ssh://git@github.ibm.com/mattgreen/gskpkgs", &metadata);
        // Should use src input's commit, not flake's own revision
        assert_eq!(info.commit, "bbb222");
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.forge_url.as_deref(), Some("https://github.ibm.com"));
        assert_eq!(info.owner.as_deref(), Some("GSKit"));
        assert_eq!(info.repo.as_deref(), Some("GSKit8-repo"));
        assert!(!info.dirty);
    }

    #[test]
    fn test_resolve_git_info_flake_revision_fallback() {
        // When flake_ref is remote and no src input, should fall back to flake revision
        let metadata = FlakeMetadata {
            description: None,
            url: "git+ssh://git@github.ibm.com/mattgreen/gskpkgs".to_string(),
            resolved_url: "git+ssh://git@github.ibm.com/mattgreen/gskpkgs".to_string(),
            revision: Some("aaa111".to_string()),
            inputs: vec![
                FlakeInput {
                    name: "nixpkgs".to_string(),
                    input_type: "github".to_string(),
                    url: "github:NixOS/nixpkgs".to_string(),
                    locked_rev: Some("ccc333".to_string()),
                    locked_ref: None,
                    last_modified: None,
                },
            ],
        };

        let info = resolve_git_info("git+ssh://git@github.ibm.com/mattgreen/gskpkgs", &metadata);
        assert_eq!(info.commit, "aaa111");
        assert_eq!(info.forge_url.as_deref(), Some("https://github.ibm.com"));
        assert_eq!(info.owner.as_deref(), Some("mattgreen"));
        assert_eq!(info.repo.as_deref(), Some("gskpkgs"));
    }

    #[test]
    fn test_resolve_git_info_no_revision_fallback() {
        // When flake_ref is remote, no src input, and no revision — should get all zeros
        let metadata = FlakeMetadata {
            description: None,
            url: "git+ssh://git@example.com/org/repo".to_string(),
            resolved_url: "git+ssh://git@example.com/org/repo".to_string(),
            revision: None,
            inputs: vec![],
        };

        let info = resolve_git_info("git+ssh://git@example.com/org/repo", &metadata);
        assert_eq!(info.commit, "0".repeat(40));
        assert_eq!(info.forge_url.as_deref(), Some("https://example.com"));
        assert_eq!(info.owner.as_deref(), Some("org"));
        assert_eq!(info.repo.as_deref(), Some("repo"));
    }
}
