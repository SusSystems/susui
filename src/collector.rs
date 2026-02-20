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

/// Get the git remote owner/repo
fn git_remote_info(dir: &str) -> (Option<String>, Option<String>) {
    let url = match run_cmd("git", &["-C", dir, "remote", "get-url", "origin"]) {
        Ok(u) => u.trim().to_string(),
        Err(_) => return (None, None),
    };

    // Strip authentication from URL: https://x-access-token:TOKEN@github.com/... → https://github.com/...
    let cleaned = if let Some(at_pos) = url.find('@') {
        if url.starts_with("https://") {
            format!("https://{}", &url[at_pos + 1..])
        } else {
            url.clone()
        }
    } else {
        url.clone()
    };

    // Parse github:owner/repo, git@github.com:owner/repo.git, https://github.com/owner/repo.git
    let stripped = cleaned
        .trim_start_matches("https://github.com/")
        .trim_start_matches("git@github.com:")
        .trim_start_matches("github:")
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .to_string();

    let parts: Vec<&str> = stripped.splitn(2, '/').collect();
    if parts.len() == 2 {
        (
            Some(parts[0].to_string()),
            Some(parts[1].to_string()),
        )
    } else {
        (None, None)
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
fn list_flake_outputs(nix: &str, flake_ref: &str) -> Result<Vec<String>> {
    let output = run_cmd(nix, &["flake", "show", flake_ref, "--json"])?;
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
pub fn collect_flake_metadata(flake_ref: &str) -> Result<FlakeMetadata> {
    let nix = nix_bin();
    let output = run_cmd(&nix, &["flake", "metadata", flake_ref, "--json"])?;
    parse_flake_metadata(&output)
}

/// Build a single derivation and return a Build record.
/// NOTE: This function is retained for testing purposes. The CLI never invokes
/// builds directly — it only evaluates and introspects the nix store.
#[allow(dead_code)]
pub fn build_derivation(
    flake_ref: &str,
    attr: &str,
    overrides: &[(String, String)],
    id: u64,
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

    // Get git info from the working directory
    let dir = if flake_ref == "." || flake_ref.starts_with("./") || flake_ref.starts_with('/') {
        flake_ref.trim_start_matches("./").to_string()
    } else {
        ".".to_string()
    };

    let branch = git_branch(&dir);
    let commit = git_commit(&dir).unwrap_or_else(|| "0".repeat(40));
    let (owner, repo) = git_remote_info(&dir);

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
        flake_ref: flake_ref.to_string(),
        pr: None,
        override_inputs,
        log: log_lines,
        drv_path: None,
        store_path: None,
        in_store: success,
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
pub fn eval_derivation(flake_ref: &str, attr: &str, id: u64) -> Build {
    let nix = nix_bin();
    let target = format!("{}#{}", flake_ref, attr);

    let start = Instant::now();
    let (success, stdout, stderr) = run_cmd_full(&nix, &["path-info", "--derivation", &target]);
    let elapsed = start.elapsed();

    let duration = format_duration(elapsed);

    // The derivation store path is the meaningful output
    let drv_path = stdout.trim().to_string();

    let dir = if flake_ref.starts_with('.') || flake_ref.starts_with('/') {
        flake_ref.to_string()
    } else {
        ".".to_string()
    };

    let branch = git_branch(&dir);
    let commit = git_commit(&dir).unwrap_or_else(|| "0".repeat(40));
    let (owner, repo) = git_remote_info(&dir);

    // If we got a .drv path back, the evaluation succeeded even if
    // the exit code was non-zero due to stderr warnings.
    let eval_ok = success || drv_path.starts_with("/nix/store/");

    if !eval_ok {
        // Evaluation itself failed — show the filtered error output
        let combined = format!("{}{}", stdout, stderr);
        let log_lines = make_log_lines(&combined);
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
            flake_ref: flake_ref.to_string(),
            pr: None,
            override_inputs: vec![],
            log: log_lines,
            drv_path: None,
            store_path: None,
            in_store: false,
        };
    }

    // Check if the output path exists in the store
    let (store_path, in_store) = check_output_in_store(&nix, &target, &drv_path);

    // Determine status from store presence
    let status = if in_store {
        BuildStatus::Passed
    } else {
        BuildStatus::Unknown
    };

    // Retrieve logs with multiple strategies
    let log_lines = enrich_eval_logs(&nix, &target, &drv_path, store_path.as_deref());

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
        flake_ref: flake_ref.to_string(),
        pr: None,
        override_inputs: vec![],
        log: log_lines,
        drv_path: Some(drv_path),
        store_path,
        in_store,
    }
}

/// Check whether a derivation's output path exists in the nix store.
/// Returns (store_path, exists).
fn check_output_in_store(nix: &str, target: &str, _drv_path: &str) -> (Option<String>, bool) {
    // Strategy 1: `nix path-info <target>` — asks for the output, not the .drv
    let (ok, stdout, _) = run_cmd_full(nix, &["path-info", target]);
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
///   4. `nix derivation show` — structured recipe as fallback
fn enrich_eval_logs(nix: &str, target: &str, drv_path: &str, store_path: Option<&str>) -> Vec<LogLine> {
    // Strategy 1: try `nix log` with the flake target ref
    if let Some(log_lines) = try_cached_log(nix, target, "flake target") {
        if !log_lines.is_empty() {
            return log_lines;
        }
    }

    // Strategy 2: try `nix log` with the .drv path
    if drv_path.starts_with("/nix/store/") {
        if let Some(log_lines) = try_cached_log(nix, drv_path, "derivation") {
            if !log_lines.is_empty() {
                return log_lines;
            }
        }
    }

    // Strategy 3: try `nix log` with the output store path
    if let Some(out_path) = store_path {
        if let Some(log_lines) = try_cached_log(nix, out_path, "output path") {
            if !log_lines.is_empty() {
                return log_lines;
            }
        }
    }

    // Strategy 4: search for a previous build log by derivation name.
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
            return combined;
        }
    }

    // Strategy 5: for check derivations of Rust projects, try `cargo test`
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
                return combined;
            }
        }
    }

    // Fall back to derivation metadata only
    derivation_info_lines(nix, target, drv_path)
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
    let src_dir = if flake_ref == "." || flake_ref.starts_with("./") || flake_ref.starts_with('/') {
        flake_ref.to_string()
    } else {
        return None; // Remote flake, can't run cargo locally
    };

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

/// Scan a flake and return builds for all discovered outputs (evaluation only, no builds triggered)
pub fn scan_flake(flake_ref: &str) -> Result<Vec<Build>> {
    let nix = nix_bin();
    tracing::info!(flake_ref, "Scanning flake outputs");

    let outputs = list_flake_outputs(&nix, flake_ref)
        .with_context(|| format!("Failed to list outputs for {}", flake_ref))?;

    tracing::info!(count = outputs.len(), "Found flake outputs");

    let mut builds = Vec::new();
    for (i, attr) in outputs.iter().enumerate() {
        tracing::info!(attr, "Processing derivation");
        let build = eval_derivation(flake_ref, attr, (i + 1) as u64);
        builds.push(build);
    }

    sort_by_dependency_order(&mut builds);

    Ok(builds)
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

/// Collect all data for a flake: metadata + builds (evaluation only, no builds triggered)
pub fn collect_all(flake_ref: &str) -> Result<(FlakeMetadata, Vec<Build>)> {
    let metadata = collect_flake_metadata(flake_ref)?;
    let builds = scan_flake(flake_ref)?;
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
                commit: "abc".into(), owner: None, repo: None,
                flake_ref: ".".into(), pr: None, override_inputs: vec![], log: vec![],
                drv_path: None, store_path: None, in_store: true,
            },
            Build {
                id: 2, derivation: "b".into(), status: BuildStatus::Failed,
                duration: "2s".into(), time: "now".into(), branch: None,
                commit: "def".into(), owner: None, repo: None,
                flake_ref: ".".into(), pr: None,
                override_inputs: vec![OverrideInput {
                    input_name: "nixpkgs".into(), input_type: "github".into(),
                    owner: Some("NixOS".into()), repo: Some("nixpkgs".into()),
                    git_ref: Some("main".into()), pr: None,
                }],
                log: vec![],
                drv_path: None, store_path: None, in_store: false,
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
            flake_ref: ".".into(),
            pr: None,
            override_inputs: vec![],
            log: vec![],
            drv_path: drv_path.map(|s| s.to_string()),
            store_path: None,
            in_store: false,
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
}
