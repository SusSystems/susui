use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level susui configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub filters: Filters,
    #[serde(default)]
    pub status_push: Vec<StatusPushTarget>,
}

/// Filter configuration for controlling which builds are displayed
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    #[serde(default)]
    pub allow: Vec<FilterRule>,
    #[serde(default)]
    pub deny: Vec<FilterRule>,
}

/// A single filter rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRule {
    /// Which flake input names this rule applies to
    pub inputs: Vec<String>,
    /// Input type: "github" (public) or "git" (enterprise)
    #[serde(rename = "type")]
    pub input_type: String,
    /// Repository owner (for github-type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Organization (for git-type enterprise)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Repository name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Hostname for enterprise instances
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Status push target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusPushTarget {
    /// Which flake input's resolved rev to report against
    pub input: String,
    /// Input type: "github" or "git"
    #[serde(rename = "type")]
    pub input_type: String,
    /// Repository owner (public GitHub)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Organization (enterprise GitHub)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Repository name
    pub repo: String,
    /// Push method: "commit_status" or "check_run"
    #[serde(default = "default_method")]
    pub method: String,
    /// Context label for commit statuses
    #[serde(default = "default_context")]
    pub context: String,
    /// Check run name (for check_run method)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_name: Option<String>,
    /// Optional URL the status links to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    /// Hostname for enterprise instances
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

fn default_method() -> String {
    "commit_status".to_string()
}

fn default_context() -> String {
    "nix-build".to_string()
}

impl Config {
    /// Load configuration from a YAML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        Ok(config)
    }

    /// Try to load from default locations, returning a default Config if not found
    pub fn load_or_default(path: Option<&str>) -> Self {
        if let Some(p) = path {
            match Config::load(Path::new(p)) {
                Ok(c) => return c,
                Err(e) => {
                    tracing::warn!("Failed to load config from {}: {}", p, e);
                }
            }
        }

        // Try default locations
        for candidate in &["susui.yaml", "susui.yml", ".susui.yaml", ".susui.yml"] {
            if let Ok(c) = Config::load(Path::new(candidate)) {
                tracing::info!("Loaded config from {}", candidate);
                return c;
            }
        }

        Config::default()
    }
}

/// Resolved input info for filter matching
#[allow(dead_code)]
pub struct ResolvedInput {
    pub name: String,
    pub input_type: String,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub host: Option<String>,
}

impl Filters {
    /// Check whether a build (represented by its resolved inputs) should be shown
    #[allow(dead_code)]
    pub fn should_show(&self, inputs: &[ResolvedInput]) -> bool {
        // If no allow filters, everything is allowed by default
        let allowed = if self.allow.is_empty() {
            true
        } else {
            self.allow.iter().any(|rule| rule_matches(rule, inputs))
        };

        if !allowed {
            return false;
        }

        // Check deny filters
        if !self.deny.is_empty() && self.deny.iter().any(|rule| rule_matches(rule, inputs)) {
            return false;
        }

        true
    }
}

fn rule_matches(rule: &FilterRule, inputs: &[ResolvedInput]) -> bool {
    for input_name in &rule.inputs {
        // Find the resolved input matching this name
        if let Some(resolved) = inputs.iter().find(|ri| &ri.name == input_name) {
            if resolved.input_type != rule.input_type {
                continue;
            }

            match rule.input_type.as_str() {
                "github" => {
                    let owner_match = rule.owner.as_ref().map_or(true, |o| {
                        resolved.owner.as_ref().map_or(false, |ro| ro == o)
                    });
                    let repo_match = rule.repo.as_ref().map_or(true, |r| {
                        resolved.repo.as_ref().map_or(false, |rr| rr == r)
                    });
                    if owner_match && repo_match {
                        return true;
                    }
                }
                "git" => {
                    let host_match = rule.host.as_ref().map_or(true, |h| {
                        resolved.host.as_ref().map_or(false, |rh| rh == h)
                    });
                    let org_match = rule.org.as_ref().map_or(true, |o| {
                        resolved.owner.as_ref().map_or(false, |ro| ro == o)
                    });
                    let repo_match = rule.repo.as_ref().map_or(true, |r| {
                        resolved.repo.as_ref().map_or(false, |rr| rr == r)
                    });
                    if host_match && org_match && repo_match {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(name: &str, itype: &str, owner: Option<&str>, repo: Option<&str>) -> ResolvedInput {
        ResolvedInput {
            name: name.to_string(),
            input_type: itype.to_string(),
            owner: owner.map(|s| s.to_string()),
            repo: repo.map(|s| s.to_string()),
            host: None,
        }
    }

    #[test]
    fn test_empty_filters_allow_all() {
        let filters = Filters::default();
        let inputs = vec![make_input("src", "github", Some("org"), Some("app"))];
        assert!(filters.should_show(&inputs));
    }

    #[test]
    fn test_allow_filter_matches() {
        let filters = Filters {
            allow: vec![FilterRule {
                inputs: vec!["src".to_string()],
                input_type: "github".to_string(),
                owner: Some("my-org".to_string()),
                org: None,
                repo: None,
                host: None,
            }],
            deny: vec![],
        };
        let inputs = vec![make_input("src", "github", Some("my-org"), Some("app"))];
        assert!(filters.should_show(&inputs));
    }

    #[test]
    fn test_allow_filter_rejects() {
        let filters = Filters {
            allow: vec![FilterRule {
                inputs: vec!["src".to_string()],
                input_type: "github".to_string(),
                owner: Some("my-org".to_string()),
                org: None,
                repo: None,
                host: None,
            }],
            deny: vec![],
        };
        let inputs = vec![make_input("src", "github", Some("other-org"), Some("app"))];
        assert!(!filters.should_show(&inputs));
    }

    #[test]
    fn test_deny_filter() {
        let filters = Filters {
            allow: vec![],
            deny: vec![FilterRule {
                inputs: vec!["src".to_string()],
                input_type: "github".to_string(),
                owner: Some("my-org".to_string()),
                org: None,
                repo: Some("scratch".to_string()),
                host: None,
            }],
        };
        let inputs = vec![make_input("src", "github", Some("my-org"), Some("scratch"))];
        assert!(!filters.should_show(&inputs));

        let inputs2 = vec![make_input("src", "github", Some("my-org"), Some("real-app"))];
        assert!(filters.should_show(&inputs2));
    }

    #[test]
    fn test_config_parse() {
        let yaml = r#"
filters:
  allow:
    - inputs: ["src"]
      type: github
      owner: my-org
  deny:
    - inputs: ["src"]
      type: github
      owner: my-org
      repo: scratch
status_push:
  - input: src
    type: github
    owner: my-org
    repo: my-app
    method: commit_status
    context: "nix-build/local"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.filters.allow.len(), 1);
        assert_eq!(config.filters.deny.len(), 1);
        assert_eq!(config.status_push.len(), 1);
        assert_eq!(config.status_push[0].context, "nix-build/local");
    }
}
