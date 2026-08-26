use davr_config::{Config, RequiredTool};
use davr_types::{CheckCategory, CheckStatus};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub category: CheckCategory,
    pub status: CheckStatus,
    pub detail: String,
    pub tool_name: Option<String>,
    pub tool_version: Option<String>,
    pub resolved_path: Option<String>,
}

pub trait LanguageAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, project_root: &Path) -> bool;
    fn recognized_lockfiles(&self) -> &[&str];
    fn default_required_tools(&self) -> Vec<RequiredTool>;
    fn infer_package_manager(&self, project_root: &Path) -> Option<String>;
}

// ---------------------------------------------------------------------
// Adapters for TypeScript/Node, Python, Rust, Go
// ---------------------------------------------------------------------

pub struct TypeScriptAdapter;
impl LanguageAdapter for TypeScriptAdapter {
    fn name(&self) -> &str {
        "typescript"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("package.json").exists()
    }

    fn recognized_lockfiles(&self) -> &[&str] {
        &[
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lockb",
        ]
    }

    fn default_required_tools(&self) -> Vec<RequiredTool> {
        vec![RequiredTool {
            name: "node".into(),
            min_version: Some("18.0.0".into()),
        }]
    }

    fn infer_package_manager(&self, project_root: &Path) -> Option<String> {
        if project_root.join("pnpm-lock.yaml").exists() {
            Some("pnpm".into())
        } else if project_root.join("yarn.lock").exists() {
            Some("yarn".into())
        } else if project_root.join("bun.lockb").exists() {
            Some("bun".into())
        } else if project_root.join("package-lock.json").exists() {
            Some("npm".into())
        } else {
            Some("npm".into())
        }
    }
}

pub struct PythonAdapter;
impl LanguageAdapter for PythonAdapter {
    fn name(&self) -> &str {
        "python"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("pyproject.toml").exists()
            || project_root.join("requirements.txt").exists()
            || project_root.join("setup.py").exists()
    }

    fn recognized_lockfiles(&self) -> &[&str] {
        &["poetry.lock", "uv.lock", "Pipfile.lock", "requirements.txt"]
    }

    fn default_required_tools(&self) -> Vec<RequiredTool> {
        vec![RequiredTool {
            name: "python3".into(),
            min_version: Some("3.10.0".into()),
        }]
    }

    fn infer_package_manager(&self, project_root: &Path) -> Option<String> {
        if project_root.join("poetry.lock").exists() {
            Some("poetry".into())
        } else if project_root.join("uv.lock").exists() {
            Some("uv".into())
        } else if project_root.join("Pipfile.lock").exists() {
            Some("pipenv".into())
        } else {
            Some("pip".into())
        }
    }
}

pub struct RustAdapter;
impl LanguageAdapter for RustAdapter {
    fn name(&self) -> &str {
        "rust"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("Cargo.toml").exists()
    }

    fn recognized_lockfiles(&self) -> &[&str] {
        &["Cargo.lock"]
    }

    fn default_required_tools(&self) -> Vec<RequiredTool> {
        vec![
            RequiredTool {
                name: "cargo".into(),
                min_version: None,
            },
            RequiredTool {
                name: "rustc".into(),
                min_version: None,
            },
        ]
    }

    fn infer_package_manager(&self, _project_root: &Path) -> Option<String> {
        Some("cargo".into())
    }
}

pub struct GoAdapter;
impl LanguageAdapter for GoAdapter {
    fn name(&self) -> &str {
        "go"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("go.mod").exists()
    }

    fn recognized_lockfiles(&self) -> &[&str] {
        &["go.sum"]
    }

    fn default_required_tools(&self) -> Vec<RequiredTool> {
        vec![RequiredTool {
            name: "go".into(),
            min_version: Some("1.20".into()),
        }]
    }

    fn infer_package_manager(&self, _project_root: &Path) -> Option<String> {
        Some("go".into())
    }
}

pub struct EnvironmentValidator {
    adapters: Vec<Box<dyn LanguageAdapter>>,
}

impl Default for EnvironmentValidator {
    fn default() -> Self {
        Self {
            adapters: vec![
                Box::new(TypeScriptAdapter),
                Box::new(PythonAdapter),
                Box::new(RustAdapter),
                Box::new(GoAdapter),
            ],
        }
    }
}

impl EnvironmentValidator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn detect_languages(&self, project_root: &Path) -> Vec<String> {
        self.adapters
            .iter()
            .filter(|a| a.detect(project_root))
            .map(|a| a.name().to_string())
            .collect()
    }

    /// Runs all pre-flight checks in fixed order per Part 5 §8.4
    pub fn validate(&self, project_root: &Path, config: &Config) -> Vec<CheckResult> {
        let mut results = Vec::new();

        // 1. OS Check
        results.push(self.check_os());

        // 2. PATH & Runtime Checks for required tools
        for tool in &config.environment.required_tools {
            results.push(self.check_tool(&tool.name, tool.min_version.as_deref()));
        }

        // 3. Package Manager & Lockfile checks for detected adapters
        for adapter in &self.adapters {
            if adapter.detect(project_root) {
                // Check package manager
                if let Some(pm) = adapter.infer_package_manager(project_root) {
                    results.push(self.check_package_manager(&pm, adapter.name()));
                }

                // Check lockfile presence
                let has_lock = adapter
                    .recognized_lockfiles()
                    .iter()
                    .any(|f| project_root.join(f).exists());

                if has_lock {
                    results.push(CheckResult {
                        name: format!("{}_lockfile_present", adapter.name()),
                        category: CheckCategory::Lockfile,
                        status: CheckStatus::Pass,
                        detail: format!("Valid lockfile found for {}", adapter.name()),
                        tool_name: None,
                        tool_version: None,
                        resolved_path: None,
                    });
                } else {
                    results.push(CheckResult {
                        name: format!("{}_lockfile_present", adapter.name()),
                        category: CheckCategory::Lockfile,
                        status: CheckStatus::Warn,
                        detail: format!(
                            "No lockfile found for {}; builds may be non-deterministic",
                            adapter.name()
                        ),
                        tool_name: None,
                        tool_version: None,
                        resolved_path: None,
                    });
                }
            }
        }

        // 4. Git Check
        results.push(self.check_git(project_root));

        // 5. Docker (if required)
        if config.environment.docker_required {
            results.push(self.check_docker());
        }

        // 6. Required Environment Variables (presence only, never value)
        for env_var in &config.environment.required_env_vars {
            results.push(self.check_env_var(env_var));
        }

        // 7. Permissions Check
        results.push(self.check_permissions(project_root));

        // 8. Repo State Check
        results.push(self.check_repo_state(project_root));

        results
    }

    fn check_os(&self) -> CheckResult {
        let os = env::consts::OS;
        let arch = env::consts::ARCH;
        CheckResult {
            name: "host_os_supported".into(),
            category: CheckCategory::Os,
            status: CheckStatus::Pass,
            detail: format!("Host platform {} ({}) is fully supported", os, arch),
            tool_name: None,
            tool_version: None,
            resolved_path: None,
        }
    }

    fn check_tool(&self, name: &str, _min_version: Option<&str>) -> CheckResult {
        match which::which(name) {
            Ok(path) => {
                // Best effort version check
                let version = probe_tool_version(name, &path);
                CheckResult {
                    name: format!("tool_{}_present", name),
                    category: CheckCategory::Runtime,
                    status: CheckStatus::Pass,
                    detail: format!("Found {} at {}", name, path.display()),
                    tool_name: Some(name.to_string()),
                    tool_version: version,
                    resolved_path: Some(path.to_string_lossy().to_string()),
                }
            }
            Err(_) => CheckResult {
                name: format!("tool_{}_present", name),
                category: CheckCategory::Path,
                status: CheckStatus::Fail,
                detail: format!("Required tool '{}' not found on PATH", name),
                tool_name: Some(name.to_string()),
                tool_version: None,
                resolved_path: None,
            },
        }
    }

    fn check_package_manager(&self, pm_name: &str, ecosystem: &str) -> CheckResult {
        match which::which(pm_name) {
            Ok(path) => CheckResult {
                name: format!("{}_package_manager_{}", ecosystem, pm_name),
                category: CheckCategory::PackageManager,
                status: CheckStatus::Pass,
                detail: format!("Package manager '{}' found at {}", pm_name, path.display()),
                tool_name: Some(pm_name.to_string()),
                tool_version: None,
                resolved_path: Some(path.to_string_lossy().to_string()),
            },
            Err(_) => CheckResult {
                name: format!("{}_package_manager_{}", ecosystem, pm_name),
                category: CheckCategory::PackageManager,
                status: CheckStatus::Fail,
                detail: format!(
                    "Package manager '{}' for {} is missing on PATH",
                    pm_name, ecosystem
                ),
                tool_name: Some(pm_name.to_string()),
                tool_version: None,
                resolved_path: None,
            },
        }
    }

    fn check_git(&self, project_root: &Path) -> CheckResult {
        let is_git_repo = project_root.join(".git").exists();
        let git_binary = which::which("git");

        if git_binary.is_ok() && is_git_repo {
            CheckResult {
                name: "git_repository_valid".into(),
                category: CheckCategory::Git,
                status: CheckStatus::Pass,
                detail: "Git binary found and project is a valid Git repository".into(),
                tool_name: Some("git".into()),
                tool_version: None,
                resolved_path: None,
            }
        } else if git_binary.is_err() {
            CheckResult {
                name: "git_binary_present".into(),
                category: CheckCategory::Git,
                status: CheckStatus::Fail,
                detail: "Git binary is missing on PATH; snapshots/rollbacks will not work".into(),
                tool_name: Some("git".into()),
                tool_version: None,
                resolved_path: None,
            }
        } else {
            CheckResult {
                name: "git_repository_initialized".into(),
                category: CheckCategory::Git,
                status: CheckStatus::Warn,
                detail: "Directory is not a Git repository; snapshot safety requires git init"
                    .into(),
                tool_name: Some("git".into()),
                tool_version: None,
                resolved_path: None,
            }
        }
    }

    fn check_docker(&self) -> CheckResult {
        if which::which("docker").is_err() {
            return CheckResult {
                name: "docker_present".into(),
                category: CheckCategory::Docker,
                status: CheckStatus::Fail,
                detail: "Docker binary not found on PATH".into(),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            };
        }

        let output = Command::new("docker").arg("info").output();
        match output {
            Ok(out) if out.status.success() => CheckResult {
                name: "docker_daemon_running".into(),
                category: CheckCategory::Docker,
                status: CheckStatus::Pass,
                detail: "Docker daemon is reachable".into(),
                tool_name: Some("docker".into()),
                tool_version: None,
                resolved_path: None,
            },
            _ => CheckResult {
                name: "docker_daemon_running".into(),
                category: CheckCategory::Docker,
                status: CheckStatus::Fail,
                detail: "Docker binary exists but daemon is not running or unreachable".into(),
                tool_name: Some("docker".into()),
                tool_version: None,
                resolved_path: None,
            },
        }
    }

    fn check_env_var(&self, name: &str) -> CheckResult {
        if env::var(name).is_ok() {
            CheckResult {
                name: format!("env_var_{}_present", name),
                category: CheckCategory::EnvVar,
                status: CheckStatus::Pass,
                detail: format!("Required environment variable '{}' is set", name),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            }
        } else {
            CheckResult {
                name: format!("env_var_{}_present", name),
                category: CheckCategory::EnvVar,
                status: CheckStatus::Fail,
                detail: format!("Required environment variable '{}' is missing", name),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            }
        }
    }

    fn check_permissions(&self, project_root: &Path) -> CheckResult {
        let test_file = project_root.join(".davr_perm_test");
        match fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = fs::remove_file(test_file);
                CheckResult {
                    name: "workspace_writable".into(),
                    category: CheckCategory::Permission,
                    status: CheckStatus::Pass,
                    detail: "Project root is writable".into(),
                    tool_name: None,
                    tool_version: None,
                    resolved_path: None,
                }
            }
            Err(e) => CheckResult {
                name: "workspace_writable".into(),
                category: CheckCategory::Permission,
                status: CheckStatus::Fail,
                detail: format!("Project root is not writable: {}", e),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            },
        }
    }

    fn check_repo_state(&self, project_root: &Path) -> CheckResult {
        if !project_root.join(".git").exists() {
            return CheckResult {
                name: "repo_clean_state".into(),
                category: CheckCategory::RepoState,
                status: CheckStatus::Skipped,
                detail: "Not a git repository".into(),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            };
        }

        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .arg("status")
            .arg("--porcelain")
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.trim().is_empty() {
                    CheckResult {
                        name: "repo_clean_state".into(),
                        category: CheckCategory::RepoState,
                        status: CheckStatus::Pass,
                        detail: "Working directory is clean".into(),
                        tool_name: None,
                        tool_version: None,
                        resolved_path: None,
                    }
                } else {
                    CheckResult {
                        name: "repo_clean_state".into(),
                        category: CheckCategory::RepoState,
                        status: CheckStatus::Warn,
                        detail: "Working directory has uncommitted changes (dirty state will be captured in pre-run snapshot)".into(),
                        tool_name: None,
                        tool_version: None,
                        resolved_path: None,
                    }
                }
            }
            _ => CheckResult {
                name: "repo_clean_state".into(),
                category: CheckCategory::RepoState,
                status: CheckStatus::Warn,
                detail: "Could not query git status".into(),
                tool_name: None,
                tool_version: None,
                resolved_path: None,
            },
        }
    }
}

fn probe_tool_version(name: &str, path: &Path) -> Option<String> {
    let flag = if name == "go" { "version" } else { "--version" };
    let output = Command::new(path).arg(flag).output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let first_line = text.lines().next().unwrap_or("").trim();
        Some(first_line.to_string())
    } else {
        None
    }
}
