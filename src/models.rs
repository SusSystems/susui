use serde::{Deserialize, Serialize};

/// Status of a nix build
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BuildStatus {
    Passed,
    Failed,
    Running,
    Pending,
    Skipped,
    /// Evaluation succeeded but output is not in the store (not yet built)
    Unknown,
}

impl std::fmt::Display for BuildStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::Running => write!(f, "running"),
            Self::Pending => write!(f, "pending"),
            Self::Skipped => write!(f, "skipped"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// A log line from a nix build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub n: usize,
    pub text: String,
    pub level: String,
}

/// An override input on a build
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideInput {
    pub input_name: String,
    #[serde(rename = "type")]
    pub input_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(rename = "ref")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
}

/// A nix build entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub id: u64,
    pub derivation: String,
    pub status: BuildStatus,
    pub duration: String,
    pub time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The forge base URL (e.g. "https://github.com", "https://gitlab.com")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge_url: Option<String>,
    pub flake_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default)]
    pub override_inputs: Vec<OverrideInput>,
    #[serde(default)]
    pub log: Vec<LogLine>,
    /// The .drv store path for this derivation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drv_path: Option<String>,
    /// The output store path (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_path: Option<String>,
    /// Whether the output is present in the nix store (i.e. previously built)
    #[serde(default)]
    pub in_store: bool,
    /// Whether this is a historical build (prior derivation, not from current flake eval)
    #[serde(default)]
    pub historical: bool,
    /// Whether the git working tree was dirty when this build was evaluated
    #[serde(default)]
    pub dirty: bool,
}

/// Summary stats for the dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStats {
    pub all: usize,
    pub passed: usize,
    pub failed: usize,
    pub running: usize,
    pub pending: usize,
    pub skipped: usize,
    pub unknown: usize,
    pub overridden: usize,
    pub in_store: usize,
    pub historical: usize,
    pub success_rate: f64,
}

impl BuildStats {
    pub fn from_builds(builds: &[Build]) -> Self {
        let all = builds.len();
        let passed = builds.iter().filter(|b| b.status == BuildStatus::Passed).count();
        let failed = builds.iter().filter(|b| b.status == BuildStatus::Failed).count();
        let running = builds.iter().filter(|b| b.status == BuildStatus::Running).count();
        let pending = builds.iter().filter(|b| b.status == BuildStatus::Pending).count();
        let skipped = builds.iter().filter(|b| b.status == BuildStatus::Skipped).count();
        let unknown = builds.iter().filter(|b| b.status == BuildStatus::Unknown).count();
        let overridden = builds.iter().filter(|b| !b.override_inputs.is_empty()).count();
        let in_store = builds.iter().filter(|b| b.in_store).count();
        let historical = builds.iter().filter(|b| b.historical).count();
        let current_count = all - historical;
        let current_passed = builds.iter().filter(|b| !b.historical && b.status == BuildStatus::Passed).count();
        let success_rate = if current_count > 0 {
            (current_passed as f64 / current_count as f64) * 100.0
        } else {
            0.0
        };
        Self {
            all,
            passed,
            failed,
            running,
            pending,
            skipped,
            unknown,
            overridden,
            in_store,
            historical,
            success_rate,
        }
    }
}

/// Flake input info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeInput {
    pub name: String,
    #[serde(rename = "type")]
    pub input_type: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// Flake metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeMetadata {
    pub description: Option<String>,
    pub url: String,
    pub resolved_url: String,
    pub revision: Option<String>,
    pub inputs: Vec<FlakeInput>,
}

/// API response wrapper
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self { ok: true, data }
    }
}
