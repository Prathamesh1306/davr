use chrono::Utc;
use davr_storage::Database;
use davr_types::{DavrError, ProjectId, Result, SessionId};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestCaseStatus {
    Passed,
    Failed,
    Skipped,
    Timeout,
    Error,
}

impl TestCaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TestCaseStatus::Passed => "passed",
            TestCaseStatus::Failed => "failed",
            TestCaseStatus::Skipped => "skipped",
            TestCaseStatus::Timeout => "timeout",
            TestCaseStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCaseResult {
    pub name: String,
    pub status: TestCaseStatus,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuiteResult {
    pub framework: String,
    pub exit_code: i32,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: i64,
    pub test_cases: Vec<TestCaseResult>,
    pub raw_output: String,
}

pub trait TestFrameworkAdapter: Send + Sync {
    fn framework_name(&self) -> &'static str;
    fn detect(&self, project_root: &Path) -> bool;
    fn build_command(&self, filter: Option<&str>) -> (String, Vec<String>);
    fn parse_output(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> TestSuiteResult;
}

// =====================================================================
// 1. Rust: Cargo Test Adapter
// =====================================================================

pub struct CargoTestAdapter;

impl TestFrameworkAdapter for CargoTestAdapter {
    fn framework_name(&self) -> &'static str {
        "cargo_test"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("Cargo.toml").exists()
    }

    fn build_command(&self, filter: Option<&str>) -> (String, Vec<String>) {
        let mut args = vec!["test".to_string()];
        if let Some(f) = filter {
            args.push("--".into());
            args.push(f.into());
        }
        ("cargo".into(), args)
    }

    fn parse_output(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> TestSuiteResult {
        let mut test_cases = Vec::new();
        let mut passed = 0;
        let mut failed = 0;
        let mut skipped = 0;

        let combined = format!("{}\n{}", stdout, stderr);
        for line in combined.lines() {
            let line = line.trim();
            if line.starts_with("test ") && (line.ends_with("... ok") || line.ends_with("... FAILED") || line.ends_with("... ignored")) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let test_name = parts[1].to_string();
                    if line.ends_with("... ok") {
                        passed += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Passed,
                            duration_ms: None,
                            error_message: None,
                        });
                    } else if line.ends_with("... FAILED") {
                        failed += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Failed,
                            duration_ms: None,
                            error_message: Some("Test failed during execution".into()),
                        });
                    } else if line.ends_with("... ignored") {
                        skipped += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Skipped,
                            duration_ms: None,
                            error_message: None,
                        });
                    }
                }
            }
        }

        let total = passed + failed + skipped;

        TestSuiteResult {
            framework: self.framework_name().into(),
            exit_code,
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            test_cases,
            raw_output: combined,
        }
    }
}

// =====================================================================
// 2. Python: Pytest Adapter
// =====================================================================

pub struct PytestAdapter;

impl TestFrameworkAdapter for PytestAdapter {
    fn framework_name(&self) -> &'static str {
        "pytest"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("pytest.ini").exists()
            || project_root.join("pyproject.toml").exists()
            || project_root.join("setup.py").exists()
            || project_root.join("requirements.txt").exists()
            || project_root.join("tests").exists()
    }

    fn build_command(&self, filter: Option<&str>) -> (String, Vec<String>) {
        let mut args = vec!["-v".to_string()];
        if let Some(f) = filter {
            args.push("-k".into());
            args.push(f.into());
        }
        ("pytest".into(), args)
    }

    fn parse_output(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> TestSuiteResult {
        let mut test_cases = Vec::new();
        let mut passed = 0;
        let mut failed = 0;
        let mut skipped = 0;

        let combined = format!("{}\n{}", stdout, stderr);
        for line in combined.lines() {
            let line = line.trim();
            if line.contains("::") && (line.ends_with("PASSED") || line.ends_with("FAILED") || line.ends_with("SKIPPED")) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(first) = parts.first() {
                    let test_name = first.to_string();
                    if line.ends_with("PASSED") {
                        passed += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Passed,
                            duration_ms: None,
                            error_message: None,
                        });
                    } else if line.ends_with("FAILED") {
                        failed += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Failed,
                            duration_ms: None,
                            error_message: Some("Assertion failure".into()),
                        });
                    } else if line.ends_with("SKIPPED") {
                        skipped += 1;
                        test_cases.push(TestCaseResult {
                            name: test_name,
                            status: TestCaseStatus::Skipped,
                            duration_ms: None,
                            error_message: None,
                        });
                    }
                }
            }
        }

        let total = passed + failed + skipped;

        TestSuiteResult {
            framework: self.framework_name().into(),
            exit_code,
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            test_cases,
            raw_output: combined,
        }
    }
}

// =====================================================================
// 3. Node/TS: Jest & Vitest Adapter
// =====================================================================

pub struct JestAdapter;

impl TestFrameworkAdapter for JestAdapter {
    fn framework_name(&self) -> &'static str {
        "jest"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("package.json").exists()
            || project_root.join("jest.config.js").exists()
            || project_root.join("jest.config.ts").exists()
            || project_root.join("vitest.config.ts").exists()
    }

    fn build_command(&self, filter: Option<&str>) -> (String, Vec<String>) {
        let mut args = vec!["test".to_string(), "--".into()];
        if let Some(f) = filter {
            args.push(f.into());
        }
        ("npm".into(), args)
    }

    fn parse_output(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> TestSuiteResult {
        let mut test_cases = Vec::new();
        let mut passed = 0;
        let mut failed = 0;
        let mut skipped = 0;

        let combined = format!("{}\n{}", stdout, stderr);
        for line in combined.lines() {
            let line = line.trim();
            if line.starts_with("✓ ") || line.starts_with("√ ") || line.starts_with("PASS ") {
                passed += 1;
                let test_name = line.trim_start_matches(|c| c == '✓' || c == '√' || c == ' ').to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Passed,
                    duration_ms: None,
                    error_message: None,
                });
            } else if line.starts_with("✕ ") || line.starts_with("× ") || line.starts_with("FAIL ") {
                failed += 1;
                let test_name = line.trim_start_matches(|c| c == '✕' || c == '×' || c == ' ').to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Failed,
                    duration_ms: None,
                    error_message: Some("Test failed".into()),
                });
            } else if line.starts_with("○ ") || line.starts_with("SKIP ") {
                skipped += 1;
                let test_name = line.trim_start_matches(|c| c == '○' || c == ' ').to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Skipped,
                    duration_ms: None,
                    error_message: None,
                });
            }
        }

        let total = passed + failed + skipped;

        TestSuiteResult {
            framework: self.framework_name().into(),
            exit_code,
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            test_cases,
            raw_output: combined,
        }
    }
}

// =====================================================================
// 4. Go: Go Test Adapter
// =====================================================================

pub struct GoTestAdapter;

impl TestFrameworkAdapter for GoTestAdapter {
    fn framework_name(&self) -> &'static str {
        "go_test"
    }

    fn detect(&self, project_root: &Path) -> bool {
        project_root.join("go.mod").exists()
    }

    fn build_command(&self, filter: Option<&str>) -> (String, Vec<String>) {
        let mut args = vec!["test".to_string(), "-v".into()];
        if let Some(f) = filter {
            args.push("-run".into());
            args.push(f.into());
        }
        args.push("./...".into());
        ("go".into(), args)
    }

    fn parse_output(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: i64,
    ) -> TestSuiteResult {
        let mut test_cases = Vec::new();
        let mut passed = 0;
        let mut failed = 0;
        let mut skipped = 0;

        let combined = format!("{}\n{}", stdout, stderr);
        for line in combined.lines() {
            let line = line.trim();
            if line.starts_with("--- PASS: ") {
                passed += 1;
                let test_name = line.trim_start_matches("--- PASS: ").split_whitespace().next().unwrap_or("").to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Passed,
                    duration_ms: None,
                    error_message: None,
                });
            } else if line.starts_with("--- FAIL: ") {
                failed += 1;
                let test_name = line.trim_start_matches("--- FAIL: ").split_whitespace().next().unwrap_or("").to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Failed,
                    duration_ms: None,
                    error_message: Some("Go test failure".into()),
                });
            } else if line.starts_with("--- SKIP: ") {
                skipped += 1;
                let test_name = line.trim_start_matches("--- SKIP: ").split_whitespace().next().unwrap_or("").to_string();
                test_cases.push(TestCaseResult {
                    name: test_name,
                    status: TestCaseStatus::Skipped,
                    duration_ms: None,
                    error_message: None,
                });
            }
        }

        let total = passed + failed + skipped;

        TestSuiteResult {
            framework: self.framework_name().into(),
            exit_code,
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            test_cases,
            raw_output: combined,
        }
    }
}

// =====================================================================
// Test Runner Engine
// =====================================================================

pub struct TestRunner {
    adapters: Vec<Box<dyn TestFrameworkAdapter>>,
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRunner {
    pub fn new() -> Self {
        Self {
            adapters: vec![
                Box::new(CargoTestAdapter),
                Box::new(PytestAdapter),
                Box::new(JestAdapter),
                Box::new(GoTestAdapter),
            ],
        }
    }

    /// Detects active test framework adapters for a project
    pub fn detect_adapters<'a>(&'a self, project_root: &Path) -> Vec<&'a Box<dyn TestFrameworkAdapter>> {
        self.adapters
            .iter()
            .filter(|adapter| adapter.detect(project_root))
            .collect()
    }

    /// Executes tests across detected or specified test frameworks
    pub async fn run(
        &self,
        project_root: &Path,
        framework_override: Option<&str>,
        filter: Option<&str>,
        timeout_seconds: u64,
    ) -> Result<Vec<TestSuiteResult>> {
        let active_adapters: Vec<&Box<dyn TestFrameworkAdapter>> = if let Some(fw) = framework_override {
            self.adapters
                .iter()
                .filter(|a| a.framework_name() == fw)
                .collect()
        } else {
            self.detect_adapters(project_root)
        };

        if active_adapters.is_empty() {
            return Err(DavrError::General(
                "No supported test framework detected in project".into(),
            ));
        }

        let mut results = Vec::new();

        for adapter in active_adapters {
            let (cmd_bin, args) = adapter.build_command(filter);
            info!(framework = adapter.framework_name(), cmd = %cmd_bin, "Running test suite");

            let start = Instant::now();
            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&args);
            cmd.current_dir(project_root);
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let timeout_fut = async move {
                if timeout_seconds > 0 {
                    sleep(Duration::from_secs(timeout_seconds)).await;
                    true
                } else {
                    std::future::pending::<bool>().await
                }
            };

            let output_res = tokio::select! {
                res = cmd.output() => {
                    match res {
                        Ok(out) => Ok(out),
                        Err(e) => Err(DavrError::General(format!("Failed to execute {}: {}", cmd_bin, e))),
                    }
                }
                _ = timeout_fut => {
                    Err(DavrError::General(format!("Test execution for {} timed out after {}s", adapter.framework_name(), timeout_seconds)))
                }
            };

            let duration_ms = start.elapsed().as_millis() as i64;

            match output_res {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let exit_code = output.status.code().unwrap_or(1);

                    let suite_result = adapter.parse_output(&stdout, &stderr, exit_code, duration_ms);
                    results.push(suite_result);
                }
                Err(e) => {
                    warn!(err = %e, framework = adapter.framework_name(), "Test execution error");
                    results.push(TestSuiteResult {
                        framework: adapter.framework_name().into(),
                        exit_code: 1,
                        total: 0,
                        passed: 0,
                        failed: 1,
                        skipped: 0,
                        duration_ms,
                        test_cases: vec![TestCaseResult {
                            name: "suite_execution".into(),
                            status: TestCaseStatus::Error,
                            duration_ms: Some(duration_ms),
                            error_message: Some(e.to_string()),
                        }],
                        raw_output: e.to_string(),
                    });
                }
            }
        }

        Ok(results)
    }

    /// Persists test run results into SQLite
    pub fn record_test_run(
        &self,
        db: &Database,
        project_id: &ProjectId,
        session_id: Option<&SessionId>,
        results: &[TestSuiteResult],
    ) -> Result<String> {
        let conn = db.inner();
        let verification_run_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();

        let all_passed = results.iter().all(|r| r.failed == 0 && r.exit_code == 0);
        let status = if all_passed { "passed" } else { "failed" };

        conn.execute(
            "INSERT INTO verification_runs (id, project_id, session_id, trigger, status, started_at, finished_at)
             VALUES (?1, ?2, ?3, 'manual', ?4, ?5, ?5)",
            rusqlite::params![
                &verification_run_id,
                project_id.as_str(),
                session_id.map(|s| s.as_str()),
                status,
                now,
            ],
        )
        .map_err(|e| DavrError::Database(e.to_string()))?;

        for suite in results {
            let test_run_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO test_runs (id, verification_run_id, framework, iteration_index, exit_code, started_at, finished_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?5)",
                rusqlite::params![
                    &test_run_id,
                    &verification_run_id,
                    &suite.framework,
                    suite.exit_code,
                    now,
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

            for tc in &suite.test_cases {
                let test_file_id = uuid::Uuid::new_v4().to_string();
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO test_files (id, project_id, file_path, framework)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        &test_file_id,
                        project_id.as_str(),
                        &tc.name,
                        &suite.framework,
                    ],
                );

                let test_case_id = uuid::Uuid::new_v4().to_string();
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO test_cases (id, test_file_id, test_name)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        &test_case_id,
                        &test_file_id,
                        &tc.name,
                    ],
                );

                let _ = conn.execute(
                    "INSERT INTO test_results (test_run_id, test_case_id, status, duration_ms, error_message)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        &test_run_id,
                        &test_case_id,
                        tc.status.as_str(),
                        tc.duration_ms,
                        tc.error_message.as_deref(),
                    ],
                );
            }
        }

        debug!(id = %verification_run_id, status = status, "Recorded test verification run in SQLite");
        Ok(verification_run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cargo_output_parser() {
        let adapter = CargoTestAdapter;
        let stdout = r#"
running 3 tests
test tests::test_one ... ok
test tests::test_two ... FAILED
test tests::test_three ... ignored

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.05s
"#;
        let result = adapter.parse_output(stdout, "", 101, 50);
        assert_eq!(result.total, 3);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_pytest_output_parser() {
        let adapter = PytestAdapter;
        let stdout = r#"
tests/test_api.py::test_login PASSED
tests/test_api.py::test_logout FAILED
tests/test_api.py::test_signup SKIPPED
"#;
        let result = adapter.parse_output(stdout, "", 1, 120);
        assert_eq!(result.total, 3);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_go_test_output_parser() {
        let adapter = GoTestAdapter;
        let stdout = r#"
=== RUN   TestLogin
--- PASS: TestLogin (0.01s)
=== RUN   TestAuth
--- FAIL: TestAuth (0.02s)
=== RUN   TestSlow
--- SKIP: TestSlow (0.00s)
"#;
        let result = adapter.parse_output(stdout, "", 1, 80);
        assert_eq!(result.total, 3);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
    }
}
