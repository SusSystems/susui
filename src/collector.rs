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

/// Build a single derivation and return a Build record
pub fn build_derivation(
    flake_ref: &str,
    attr: &str,
    overrides: &[(String, String)],
    id: u64,
) -> Build {
    let nix = nix_bin();
    let target = format!("{}#{}", flake_ref, attr);

    let mut args = vec!["build", &target, "--no-link"];
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
pub fn eval_derivation(flake_ref: &str, attr: &str, id: u64) -> Build {
    let nix = nix_bin();
    let target = format!("{}#{}", flake_ref, attr);

    let start = Instant::now();
    let (success, stdout, stderr) = run_cmd_full(&nix, &["path-info", "--derivation", &target]);
    let elapsed = start.elapsed();

    let duration = format_duration(elapsed);

    // The derivation store path is the meaningful output
    let drv_path = stdout.trim().to_string();
    let combined = format!("{}{}", stdout, stderr);
    let log_lines = make_log_lines(&combined);

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

/// Scan a flake and return builds for all discovered outputs
pub fn scan_flake(flake_ref: &str, dry_run: bool) -> Result<Vec<Build>> {
    let nix = nix_bin();
    tracing::info!(flake_ref, "Scanning flake outputs");

    let outputs = list_flake_outputs(&nix, flake_ref)
        .with_context(|| format!("Failed to list outputs for {}", flake_ref))?;

    tracing::info!(count = outputs.len(), "Found flake outputs");

    let mut builds = Vec::new();
    for (i, attr) in outputs.iter().enumerate() {
        tracing::info!(attr, "Processing derivation");
        let build = if dry_run {
            eval_derivation(flake_ref, attr, (i + 1) as u64)
        } else {
            build_derivation(flake_ref, attr, &[], (i + 1) as u64)
        };
        builds.push(build);
    }

    Ok(builds)
}

/// Collect all data for a flake: metadata + builds
pub fn collect_all(flake_ref: &str, dry_run: bool) -> Result<(FlakeMetadata, Vec<Build>)> {
    let metadata = collect_flake_metadata(flake_ref)?;
    let builds = scan_flake(flake_ref, dry_run)?;
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
