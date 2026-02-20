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

    // Parse github:owner/repo, git@github.com:owner/repo.git, https://github.com/owner/repo.git
    let stripped = url
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
    }
}

/// Evaluate a derivation (dry-run) and return a Build record.
///
/// Uses `nix path-info --derivation` which only needs to evaluate the
/// nix expression — it does NOT require the full dependency closure to be
/// present in the store, so it works reliably in minimal / container
/// environments where `nix build --dry-run` would fail with missing .drv
/// errors.
///
/// After resolving the .drv path, enriches the log with:
///   1. `nix log <drv>` — cached build output if the derivation was previously built
///   2. `nix derivation show` — structured build recipe (builder, phases, inputs, outputs)
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

    // Enrich logs: try cached build log first, then derivation metadata
    let log_lines = if eval_ok {
        enrich_eval_logs(&nix, &target, &drv_path)
    } else {
        // Evaluation itself failed — show the filtered error output
        let combined = format!("{}{}", stdout, stderr);
        make_log_lines(&combined)
    };

    Build {
        id,
        derivation: attr.to_string(),
        status: if eval_ok {
            BuildStatus::Passed
        } else {
            BuildStatus::Failed
        },
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
    }
}

/// Enrich eval-mode logs with cached build output or derivation metadata.
///
/// Priority:
///   1. `nix log <drv>` — real build output from a previous build
///   2. `nix derivation show` — structured recipe (builder, phases, deps, outputs)
fn enrich_eval_logs(nix: &str, target: &str, drv_path: &str) -> Vec<LogLine> {
    // Try cached build log first
    if let Some(log_lines) = try_cached_log(nix, drv_path) {
        if !log_lines.is_empty() {
            return log_lines;
        }
    }

    // Fall back to derivation metadata
    derivation_info_lines(nix, target, drv_path)
}

/// Try to fetch a cached build log via `nix log`.
fn try_cached_log(nix: &str, drv_path: &str) -> Option<Vec<LogLine>> {
    let (ok, stdout, _stderr) = run_cmd_full(nix, &["log", drv_path]);
    if !ok || stdout.trim().is_empty() {
        return None;
    }

    let lines = make_log_lines(&stdout);
    if lines.is_empty() {
        return None;
    }

    // Prepend a header so it's clear this is a cached log
    let mut result = vec![LogLine {
        n: 1,
        text: format!("─── cached build log ({}) ───", drv_path.split('/').last().unwrap_or(drv_path)),
        level: "dim".to_string(),
    }];

    for (i, mut line) in lines.into_iter().enumerate() {
        line.n = i + 2;
        result.push(line);
    }

    Some(result)
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
                    let short = builder.split('/').last().unwrap_or(builder);
                    lines.push((format!("builder:  {}", short), "info".to_string()));
                }

                if let Some(env_map) = env {
                    // Key build metadata
                    for key in &["pname", "version", "src", "cargoDeps", "cargoBuildType"] {
                        if let Some(val) = env_map.get(*key).and_then(|v| v.as_str()) {
                            let display = if val.starts_with("/nix/store/") {
                                val.split('/').last().unwrap_or(val)
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
                                .filter_map(|p| p.split('/').last())
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

    Ok(builds)
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
    if l.contains("error") || l.contains("failed") || l.contains("fail:") {
        "error".to_string()
    } else if l.contains("warning") || l.contains("override-input") {
        "warning".to_string()
    } else if l.contains("success") || l.contains("built successfully") || l.starts_with("/nix/store/") {
        "success".to_string()
    } else if l.contains("evaluating") || l.contains("copying") || l.starts_with("building") {
        "nix".to_string()
    } else if l.starts_with("  ") || l.contains("...") {
        "dim".to_string()
    } else {
        "info".to_string()
    }
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
            },
        ];
        let stats = BuildStats::from_builds(&builds);
        assert_eq!(stats.all, 2);
        assert_eq!(stats.passed, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.overridden, 1);
        assert!((stats.success_rate - 50.0).abs() < 0.01);
    }
}
