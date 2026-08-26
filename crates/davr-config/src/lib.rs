use davr_types::{Confidence, DavrError, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub project: ProjectConfig,
    pub environment: EnvironmentConfig,
    pub agent: AgentConfig,
    pub security: SecurityConfig,
    pub git: GitConfig,
    pub telemetry: TelemetryConfig,
    pub test: TestConfig,
    pub flaky: FlakyConfig,
    pub mcp: McpConfig,
    pub ci: CiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    #[serde(default)]
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequiredTool {
    pub name: String,
    #[serde(default)]
    pub min_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvironmentConfig {
    #[serde(default)]
    pub required_tools: Vec<RequiredTool>,
    #[serde(default)]
    pub required_env_vars: Vec<String>,
    #[serde(default)]
    pub required_credentials: Vec<String>,
    #[serde(default)]
    pub docker_required: bool,
    #[serde(default)]
    pub warnings_block_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentConfig {
    pub default_agent: String,
    #[serde(default)]
    pub allowed_agents: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub sanitize_env: bool,
    #[serde(default = "default_env_allowlist")]
    pub env_allowlist: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_env_allowlist() -> Vec<String> {
    vec![
        "PATH".into(),
        "HOME".into(),
        "LANG".into(),
        "ANTHROPIC_API_KEY".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConfig {
    #[serde(default = "default_blocked_commands")]
    pub blocked_commands: Vec<String>,
    #[serde(default = "default_confirm_commands")]
    pub confirm_commands: Vec<String>,
    #[serde(default = "default_redact_patterns")]
    pub redact_patterns: Vec<String>,
}

fn default_blocked_commands() -> Vec<String> {
    vec![
        r"regex:^rm\s+-rf\s+/(\s|$)".into(),
        "glob:git push --force*".into(),
    ]
}

fn default_confirm_commands() -> Vec<String> {
    vec!["glob:git push*".into(), "glob:*DROP TABLE*".into()]
}

fn default_redact_patterns() -> Vec<String> {
    vec![
        "regex:sk-[A-Za-z0-9]{20,}".into(),
        "regex:ghp_[A-Za-z0-9]{36}".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitConfig {
    #[serde(default = "default_true")]
    pub snapshot_on_run: bool,
    #[serde(default = "default_max_snapshots")]
    pub max_snapshots_per_project: usize,
    #[serde(default = "default_retention_days_14")]
    pub snapshot_retention_days: u32,
}

fn default_max_snapshots() -> usize {
    20
}
fn default_retention_days_14() -> u32 {
    14
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_retention_days_30")]
    pub retention_days: u32,
    #[serde(default = "default_retention_days_90")]
    pub verification_retention_days: u32,
}

fn default_retention_days_30() -> u32 {
    30
}
fn default_retention_days_90() -> u32 {
    90
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestConfig {
    #[serde(default = "default_test_frameworks")]
    pub frameworks: Vec<String>,
    #[serde(default = "default_min_confidence")]
    pub impact_min_confidence: Confidence,
    #[serde(default = "default_true")]
    pub fallback_to_full_suite: bool,
    #[serde(default)]
    pub parallelism: usize,
}

fn default_test_frameworks() -> Vec<String> {
    vec![
        "pytest".into(),
        "jest".into(),
        "cargo_test".into(),
        "go_test".into(),
    ]
}

fn default_min_confidence() -> Confidence {
    Confidence::Medium
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlakyConfig {
    #[serde(default = "default_iterations")]
    pub iterations: u32,
    #[serde(default = "default_timeout_30")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_parallel")]
    pub max_parallel_iterations: usize,
}

fn default_iterations() -> u32 {
    10
}
fn default_timeout_30() -> u64 {
    30
}
fn default_max_parallel() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_mutating_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CiConfig {
    #[serde(default)]
    pub fail_on_flaky: bool,
    #[serde(default = "default_true")]
    pub post_pr_comment: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project: ProjectConfig {
                name: "my-project".into(),
                languages: vec!["typescript".into(), "python".into()],
            },
            environment: EnvironmentConfig {
                required_tools: vec![
                    RequiredTool {
                        name: "node".into(),
                        min_version: Some("18.0.0".into()),
                    },
                    RequiredTool {
                        name: "python".into(),
                        min_version: Some("3.10.0".into()),
                    },
                    RequiredTool {
                        name: "git".into(),
                        min_version: Some("2.30.0".into()),
                    },
                ],
                required_env_vars: vec!["ANTHROPIC_API_KEY".into()],
                required_credentials: vec![],
                docker_required: false,
                warnings_block_run: false,
            },
            agent: AgentConfig {
                default_agent: "claude".into(),
                allowed_agents: vec!["claude".into(), "aider".into(), "opencode".into()],
                timeout_seconds: 0,
                sanitize_env: true,
                env_allowlist: default_env_allowlist(),
            },
            security: SecurityConfig {
                blocked_commands: default_blocked_commands(),
                confirm_commands: default_confirm_commands(),
                redact_patterns: default_redact_patterns(),
            },
            git: GitConfig {
                snapshot_on_run: true,
                max_snapshots_per_project: 20,
                snapshot_retention_days: 14,
            },
            telemetry: TelemetryConfig {
                enabled: true,
                retention_days: 30,
                verification_retention_days: 90,
            },
            test: TestConfig {
                frameworks: default_test_frameworks(),
                impact_min_confidence: Confidence::Medium,
                fallback_to_full_suite: true,
                parallelism: 0,
            },
            flaky: FlakyConfig {
                iterations: 10,
                timeout_seconds: 30,
                max_parallel_iterations: 4,
            },
            mcp: McpConfig {
                enabled: false,
                allow_mutating_tools: false,
            },
            ci: CiConfig {
                fail_on_flaky: false,
                post_pr_comment: true,
            },
        }
    }
}

impl Config {
    /// Loads configuration with precedence: Defaults -> File -> Env vars -> CLI overrides
    pub fn load_from_dir(project_root: impl AsRef<Path>) -> Result<Self> {
        let config_file = project_root.as_ref().join(".davr").join("config.toml");
        let mut config = if config_file.exists() {
            let content = fs::read_to_string(&config_file)
                .map_err(|e| DavrError::Config(format!("Failed to read {}: {}", config_file.display(), e)))?;
            toml::from_str::<Config>(&content)
                .map_err(|e| DavrError::Config(format!("Failed to parse {}: {}", config_file.display(), e)))?
        } else {
            Config::default()
        };

        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Overrides configuration via DAVR_* environment variables (using __ for nesting)
    pub fn apply_env_overrides(&mut self) {
        if let Ok(val) = env::var("DAVR_AGENT__TIMEOUT_SECONDS") {
            if let Ok(parsed) = val.parse::<u64>() {
                self.agent.timeout_seconds = parsed;
            }
        }
        if let Ok(val) = env::var("DAVR_TELEMETRY__ENABLED") {
            if let Ok(parsed) = val.parse::<bool>() {
                self.telemetry.enabled = parsed;
            }
        }
        if let Ok(val) = env::var("DAVR_TEST__IMPACT_MIN_CONFIDENCE") {
            match val.to_lowercase().as_str() {
                "high" => self.test.impact_min_confidence = Confidence::High,
                "medium" => self.test.impact_min_confidence = Confidence::Medium,
                "low" => self.test.impact_min_confidence = Confidence::Low,
                _ => {}
            }
        }
    }

    /// Validates security patterns and configuration invariants
    pub fn validate(&self) -> Result<()> {
        for pattern in &self.security.blocked_commands {
            validate_pattern(pattern)?;
        }
        for pattern in &self.security.confirm_commands {
            validate_pattern(pattern)?;
        }
        for pattern in &self.security.redact_patterns {
            validate_pattern(pattern)?;
        }
        Ok(())
    }

    /// Returns the standard TOML string representation
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| DavrError::Config(e.to_string()))
    }
}

fn validate_pattern(pattern: &str) -> Result<()> {
    if let Some(re) = pattern.strip_prefix("regex:") {
        Regex::new(re).map_err(|e| DavrError::Config(format!("Invalid regex pattern '{}': {}", re, e)))?;
    } else if let Some(gl) = pattern.strip_prefix("glob:") {
        glob::Pattern::new(gl)
            .map_err(|e| DavrError::Config(format!("Invalid glob pattern '{}': {}", gl, e)))?;
    } else {
        // Default to glob if unadorned
        glob::Pattern::new(pattern)
            .map_err(|e| DavrError::Config(format!("Invalid pattern '{}': {}", pattern, e)))?;
    }
    Ok(())
}

/// Discovers the nearest directory containing `.davr/` or returns current directory
pub fn find_project_root() -> PathBuf {
    let mut current = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        if current.join(".davr").is_dir() {
            return current;
        }
        if !current.pop() {
            break;
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validation() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_pattern_fails_validation() {
        let mut config = Config::default();
        config.security.blocked_commands.push("regex:[invalid".into());
        assert!(config.validate().is_err());
    }
}
