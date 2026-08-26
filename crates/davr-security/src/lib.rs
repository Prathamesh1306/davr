use davr_config::SecurityConfig;
use davr_types::{DavrError, PolicyDecision, Result};
use glob::Pattern as GlobPattern;
use regex::Regex;
use std::path::{Path, PathBuf};

enum Matcher {
    Regex(Regex),
    Glob(GlobPattern),
}

impl Matcher {
    fn from_pattern(pattern: &str) -> Result<Self> {
        if let Some(re) = pattern.strip_prefix("regex:") {
            let compiled = Regex::new(re)
                .map_err(|e| DavrError::Security(format!("Invalid regex pattern '{}': {}", re, e)))?;
            Ok(Self::Regex(compiled))
        } else if let Some(gl) = pattern.strip_prefix("glob:") {
            let compiled = GlobPattern::new(gl)
                .map_err(|e| DavrError::Security(format!("Invalid glob pattern '{}': {}", gl, e)))?;
            Ok(Self::Glob(compiled))
        } else if pattern.contains(r"\s") || pattern.contains(r"\d") || pattern.contains(r"\w") || pattern.contains(".*") {
            let compiled = Regex::new(pattern)
                .map_err(|e| DavrError::Security(format!("Invalid pattern '{}': {}", pattern, e)))?;
            Ok(Self::Regex(compiled))
        } else {
            let compiled = GlobPattern::new(pattern)
                .map_err(|e| DavrError::Security(format!("Invalid pattern '{}': {}", pattern, e)))?;
            Ok(Self::Glob(compiled))
        }
    }

    fn is_match(&self, input: &str) -> bool {
        match self {
            Self::Regex(re) => re.is_match(input),
            Self::Glob(gl) => gl.matches(input),
        }
    }
}

pub struct SecurityEngine {
    blocked: Vec<Matcher>,
    confirm: Vec<Matcher>,
    redact: Vec<Regex>,
}

impl SecurityEngine {
    pub fn from_config(config: &SecurityConfig) -> Result<Self> {
        let mut blocked = Vec::new();
        for pattern in &config.blocked_commands {
            blocked.push(Matcher::from_pattern(pattern)?);
        }

        let mut confirm = Vec::new();
        for pattern in &config.confirm_commands {
            confirm.push(Matcher::from_pattern(pattern)?);
        }

        let mut redact = Vec::new();
        for pattern in &config.redact_patterns {
            let re_str = pattern.strip_prefix("regex:").unwrap_or(pattern);
            let compiled = Regex::new(re_str)
                .map_err(|e| DavrError::Security(format!("Invalid redact pattern '{}': {}", re_str, e)))?;
            redact.push(compiled);
        }

        Ok(Self {
            blocked,
            confirm,
            redact,
        })
    }

    /// Evaluates a raw shell command against blocked and confirm rules.
    pub fn evaluate_command(&self, command: &str) -> PolicyDecision {
        let trimmed = command.trim();
        for matcher in &self.blocked {
            if matcher.is_match(trimmed) {
                return PolicyDecision::Blocked;
            }
        }

        for matcher in &self.confirm {
            if matcher.is_match(trimmed) {
                return PolicyDecision::ConfirmedByUser;
            }
        }

        PolicyDecision::Allowed
    }

    /// Redacts known secret patterns from an input string before logging/storage.
    pub fn redact_secrets(&self, input: &str) -> String {
        let mut sanitized = input.to_string();
        for re in &self.redact {
            sanitized = re.replace_all(&sanitized, "[REDACTED]").to_string();
        }
        sanitized
    }

    /// Verifies path containment within the project root (prevents path traversal & symlink breakout).
    pub fn check_path_containment(project_root: &Path, target_path: &Path) -> Result<PathBuf> {
        let root_canonical = project_root
            .canonicalize()
            .map_err(|e| DavrError::Security(format!("Cannot canonicalize project root: {}", e)))?;

        let target_canonical = if target_path.exists() {
            target_path
                .canonicalize()
                .map_err(|e| DavrError::Security(format!("Cannot canonicalize path: {}", e)))?
        } else if let Some(parent) = target_path.parent() {
            let parent_canonical = parent
                .canonicalize()
                .map_err(|e| DavrError::Security(format!("Cannot canonicalize parent path: {}", e)))?;
            let file_name = target_path.file_name().ok_or_else(|| {
                DavrError::Security("Target path has no valid filename".to_string())
            })?;
            parent_canonical.join(file_name)
        } else {
            return Err(DavrError::Security(format!(
                "Path '{}' has no valid parent",
                target_path.display()
            )));
        };

        if target_canonical.starts_with(&root_canonical) {
            Ok(target_canonical)
        } else {
            Err(DavrError::Security(format!(
                "Path traversal rejected: '{}' resolves outside project root '{}'",
                target_path.display(),
                project_root.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_evaluation() {
        let config = SecurityConfig {
            blocked_commands: vec![r"regex:^rm\s+-rf\s+/".into(), "glob:git push --force*".into()],
            confirm_commands: vec!["glob:git push*".into()],
            redact_patterns: vec!["regex:sk-[A-Za-z0-9]{20,}".into()],
        };

        let engine = SecurityEngine::from_config(&config).unwrap();

        assert_eq!(
            engine.evaluate_command("rm -rf /"),
            PolicyDecision::Blocked
        );
        assert_eq!(
            engine.evaluate_command("git push --force origin main"),
            PolicyDecision::Blocked
        );
        assert_eq!(
            engine.evaluate_command("git push origin main"),
            PolicyDecision::ConfirmedByUser
        );
        assert_eq!(
            engine.evaluate_command("cargo test"),
            PolicyDecision::Allowed
        );
    }

    #[test]
    fn test_secret_redaction() {
        let config = SecurityConfig {
            blocked_commands: vec![],
            confirm_commands: vec![],
            redact_patterns: vec!["regex:sk-[A-Za-z0-9]{20,}".into()],
        };

        let engine = SecurityEngine::from_config(&config).unwrap();
        let sensitive = "export OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz123456";
        let redacted = engine.redact_secrets(sensitive);
        assert_eq!(redacted, "export OPENAI_API_KEY=[REDACTED]");
    }
}
