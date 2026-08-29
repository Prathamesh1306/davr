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
            let content = fs::read_to_string(&config_file).map_err(|e| {
                DavrError::Config(format!("Failed to read {}: {}", config_file.display(), e))
            })?;
            toml::from_str::<Config>(&content).map_err(|e| {
                DavrError::Config(format!("Failed to parse {}: {}", config_file.display(), e))
            })?
        } else {
            Config::default()
        };

        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Overrides configuration via DAVR_* environment variables (using __ for nesting, e.g. DAVR_AGENT__TIMEOUT_SECONDS=120)
    pub fn apply_env_overrides(&mut self) {
        for (key, val) in env::vars() {
            if let Some(stripped) = key.strip_prefix("DAVR_") {
                let dotted = stripped.replace("__", ".").to_lowercase();
                let _ = self.apply_single_override(&dotted, &val);
            }
        }
    }

    fn apply_single_override(&mut self, dotted: &str, raw_val: &str) -> Result<()> {
        match dotted {
            "agent.timeout_seconds" => {
                if let Ok(p) = raw_val.parse::<u64>() {
                    self.agent.timeout_seconds = p;
                }
            }
            "agent.default_agent" => {
                self.agent.default_agent = raw_val.to_string();
            }
            "agent.sanitize_env" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.agent.sanitize_env = b;
                }
            }
            "telemetry.enabled" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.telemetry.enabled = b;
                }
            }
            "telemetry.retention_days" => {
                if let Ok(p) = raw_val.parse::<u32>() {
                    self.telemetry.retention_days = p;
                }
            }
            "telemetry.verification_retention_days" => {
                if let Ok(p) = raw_val.parse::<u32>() {
                    self.telemetry.verification_retention_days = p;
                }
            }
            "git.snapshot_on_run" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.git.snapshot_on_run = b;
                }
            }
            "git.max_snapshots_per_project" => {
                if let Ok(p) = raw_val.parse::<usize>() {
                    self.git.max_snapshots_per_project = p;
                }
            }
            "git.snapshot_retention_days" => {
                if let Ok(p) = raw_val.parse::<u32>() {
                    self.git.snapshot_retention_days = p;
                }
            }
            "environment.docker_required" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.environment.docker_required = b;
                }
            }
            "environment.warnings_block_run" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.environment.warnings_block_run = b;
                }
            }
            "test.impact_min_confidence" => match raw_val.to_lowercase().as_str() {
                "high" => self.test.impact_min_confidence = Confidence::High,
                "medium" => self.test.impact_min_confidence = Confidence::Medium,
                "low" => self.test.impact_min_confidence = Confidence::Low,
                _ => {}
            },
            "test.fallback_to_full_suite" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.test.fallback_to_full_suite = b;
                }
            }
            "test.parallelism" => {
                if let Ok(p) = raw_val.parse::<usize>() {
                    self.test.parallelism = p;
                }
            }
            "flaky.iterations" => {
                if let Ok(p) = raw_val.parse::<u32>() {
                    self.flaky.iterations = p;
                }
            }
            "flaky.timeout_seconds" => {
                if let Ok(p) = raw_val.parse::<u64>() {
                    self.flaky.timeout_seconds = p;
                }
            }
            "ci.fail_on_flaky" => {
                if let Ok(b) = raw_val.parse::<bool>() {
                    self.ci.fail_on_flaky = b;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Retrieves the serialized string value for a dotted configuration key (e.g. "agent.default_agent")
    pub fn get_value(&self, dotted_key: &str) -> Result<String> {
        let toml_str = self.to_toml_string()?;
        let value: toml::Value =
            toml::from_str(&toml_str).map_err(|e| DavrError::Config(e.to_string()))?;

        let mut current = &value;
        for part in dotted_key.split('.') {
            match current.get(part) {
                Some(next) => current = next,
                None => {
                    return Err(DavrError::Config(format!(
                        "Key not found in configuration: {}",
                        dotted_key
                    )))
                }
            }
        }

        match current {
            toml::Value::String(s) => Ok(s.clone()),
            _ => Ok(current.to_string()),
        }
    }

    /// Sets the value for a dotted configuration key, re-validates, and saves to the project config file
    pub fn set_value(
        &mut self,
        project_root: impl AsRef<Path>,
        dotted_key: &str,
        raw_value: &str,
    ) -> Result<()> {
        let toml_str = self.to_toml_string()?;
        let mut value: toml::Value =
            toml::from_str(&toml_str).map_err(|e| DavrError::Config(e.to_string()))?;

        let parts: Vec<&str> = dotted_key.split('.').collect();
        if parts.is_empty() {
            return Err(DavrError::Config("Empty configuration key".into()));
        }

        let parsed_val: toml::Value = if raw_value == "true" {
            toml::Value::Boolean(true)
        } else if raw_value == "false" {
            toml::Value::Boolean(false)
        } else if let Ok(i) = raw_value.parse::<i64>() {
            toml::Value::Integer(i)
        } else if let Ok(f) = raw_value.parse::<f64>() {
            toml::Value::Float(f)
        } else if raw_value.starts_with('[') && raw_value.ends_with(']') {
            toml::from_str(raw_value).unwrap_or_else(|_| toml::Value::String(raw_value.into()))
        } else {
            toml::Value::String(raw_value.into())
        };

        // Navigate to the parent table
        let mut current = &mut value;
        for &part in &parts[..parts.len() - 1] {
            match current {
                toml::Value::Table(ref mut map) => {
                    current = map
                        .entry(part.to_string())
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                }
                _ => {
                    return Err(DavrError::Config(format!(
                        "Invalid path for key: {}",
                        dotted_key
                    )))
                }
            }
        }

        // Set the leaf value
        let leaf = parts[parts.len() - 1];
        if let toml::Value::Table(ref mut map) = current {
            map.insert(leaf.to_string(), parsed_val);
        } else {
            return Err(DavrError::Config(format!(
                "Invalid path for key: {}",
                dotted_key
            )));
        }

        // Serialize and re-parse into Config to ensure schema validity
        let updated_toml = toml::to_string_pretty(&value)
            .map_err(|e| DavrError::Config(format!("Failed to serialize TOML: {}", e)))?;
        let new_config: Config = toml::from_str(&updated_toml).map_err(|e| {
            DavrError::Config(format!(
                "Invalid configuration value for '{}': {}",
                dotted_key, e
            ))
        })?;

        new_config.validate()?;
        *self = new_config;

        // Write to .davr/config.toml
        let config_file = project_root.as_ref().join(".davr").join("config.toml");
        if config_file.exists() {
            fs::write(&config_file, self.to_toml_string()?)
                .map_err(|e| DavrError::Config(format!("Failed to write config file: {}", e)))?;
        }

        Ok(())
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
        Regex::new(re)
            .map_err(|e| DavrError::Config(format!("Invalid regex pattern '{}': {}", re, e)))?;
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
        config
            .security
            .blocked_commands
            .push("regex:[invalid".into());
        assert!(config.validate().is_err());
    }
}
