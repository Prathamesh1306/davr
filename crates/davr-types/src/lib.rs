use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

// =====================================================================
// Domain Identifiers
// =====================================================================

macro_rules! define_uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn from_string(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                Ok(Self(s.to_string()))
            }
        }
    };
}

define_uuid_id!(ProjectId);
define_uuid_id!(SessionId);
define_uuid_id!(SnapshotId);
define_uuid_id!(RunId);
define_uuid_id!(RollbackId);
define_uuid_id!(SourceFileId);
define_uuid_id!(SourceSymbolId);
define_uuid_id!(TestFileId);
define_uuid_id!(TestCaseId);
define_uuid_id!(VerificationRunId);

/// State of a file in the workspace: Present with a BLAKE3 content hash, or Missing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "hash", rename_all = "snake_case")]
pub enum FileState {
    Missing,
    Present(String),
}

impl FileState {
    pub fn is_present(&self) -> bool {
        matches!(self, FileState::Present(_))
    }

    pub fn hash(&self) -> Option<&str> {
        match self {
            FileState::Present(h) => Some(h.as_str()),
            FileState::Missing => None,
        }
    }
}
define_uuid_id!(TestRunId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IterationId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvironmentCheckId(pub i64);

// =====================================================================
// Enums matching SQLite Schema CHECK constraints
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    Aborted,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Aborted => write!(f, "aborted"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allowed,
    Blocked,
    ConfirmedByUser,
}

impl fmt::Display for PolicyDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::Blocked => write!(f, "blocked"),
            Self::ConfirmedByUser => write!(f, "confirmed_by_user"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    PreRun,
    PreMutation,
    Manual,
    PreRollback,
}

impl fmt::Display for SnapshotReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreRun => write!(f, "pre_run"),
            Self::PreMutation => write!(f, "pre_mutation"),
            Self::Manual => write!(f, "manual"),
            Self::PreRollback => write!(f, "pre_rollback"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckCategory {
    Os,
    Path,
    Runtime,
    PackageManager,
    Lockfile,
    Git,
    Docker,
    EnvVar,
    Credential,
    Permission,
    RepoState,
}

impl fmt::Display for CheckCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Os => write!(f, "os"),
            Self::Path => write!(f, "path"),
            Self::Runtime => write!(f, "runtime"),
            Self::PackageManager => write!(f, "package_manager"),
            Self::Lockfile => write!(f, "lockfile"),
            Self::Git => write!(f, "git"),
            Self::Docker => write!(f, "docker"),
            Self::EnvVar => write!(f, "env_var"),
            Self::Credential => write!(f, "credential"),
            Self::Permission => write!(f, "permission"),
            Self::RepoState => write!(f, "repo_state"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
    Skipped,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass => write!(f, "pass"),
            Self::Fail => write!(f, "fail"),
            Self::Warn => write!(f, "warn"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Debug => write!(f, "debug"),
            Self::Info => write!(f, "info"),
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Typescript,
    Javascript,
    Python,
    Rust,
    Go,
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Typescript => write!(f, "typescript"),
            Self::Javascript => write!(f, "javascript"),
            Self::Python => write!(f, "python"),
            Self::Rust => write!(f, "rust"),
            Self::Go => write!(f, "go"),
        }
    }
}

impl FromStr for Language {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "typescript" | "ts" => Ok(Self::Typescript),
            "javascript" | "js" => Ok(Self::Javascript),
            "python" | "py" => Ok(Self::Python),
            "rust" | "rs" => Ok(Self::Rust),
            "go" | "golang" => Ok(Self::Go),
            _ => Err(format!("Unknown language: {}", s)),
        }
    }
}

// =====================================================================
// Error Taxonomy (thiserror)
// =====================================================================

#[derive(Error, Debug)]
pub enum DavrError {
    #[error("Environment error (exit 10-19): {0}")]
    Environment(String),

    #[error("Security policy violation (exit 20-29): {0}")]
    Security(String),

    #[error("Git or snapshot error (exit 30-39): {0}")]
    Git(String),

    #[error("Database error (exit 40-49): {0}")]
    Database(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Agent supervision error: {0}")]
    Agent(String),

    #[error("General error: {0}")]
    General(String),
}

impl DavrError {
    pub fn exit_code(&self) -> i32 {
        match self {
            DavrError::General(_) => 1,
            DavrError::Environment(_) => 10,
            DavrError::Security(_) => 20,
            DavrError::Git(_) => 30,
            DavrError::Database(_) => 40,
            DavrError::Config(_) => 2,
            DavrError::Agent(_) => 50,
        }
    }
}

pub type Result<T> = std::result::Result<T, DavrError>;
