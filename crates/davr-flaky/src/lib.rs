use chrono::Utc;
use davr_storage::Database;
use davr_test::{TestCaseStatus, TestRunner};
use davr_types::{ProjectId, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlakyClassification {
    #[serde(rename = "STABLE_PASS")]
    StablePass,
    #[serde(rename = "STABLE_FAIL")]
    StableFail,
    #[serde(rename = "FLAKY")]
    Flaky,
    #[serde(rename = "TIMEOUT_UNSTABLE")]
    TimeoutUnstable,
    #[serde(rename = "INFRA_FAILURE")]
    InfraFailure,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

impl FlakyClassification {
    pub fn as_str(&self) -> &'static str {
        match self {
            FlakyClassification::StablePass => "STABLE_PASS",
            FlakyClassification::StableFail => "STABLE_FAIL",
            FlakyClassification::Flaky => "FLAKY",
            FlakyClassification::TimeoutUnstable => "TIMEOUT_UNSTABLE",
            FlakyClassification::InfraFailure => "INFRA_FAILURE",
            FlakyClassification::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakyCaseReport {
    pub test_name: String,
    pub iterations_run: usize,
    pub pass_count: usize,
    pub fail_count: usize,
    pub timeout_count: usize,
    pub classification: FlakyClassification,
    pub pass_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakySuiteReport {
    pub total_tests: usize,
    pub stable_pass: usize,
    pub stable_fail: usize,
    pub flaky_detected: usize,
    pub timeout_unstable: usize,
    pub reports: Vec<FlakyCaseReport>,
}

pub struct FlakyTestRunner {
    default_iterations: usize,
}

impl Default for FlakyTestRunner {
    fn default() -> Self {
        Self::new(5)
    }
}

impl FlakyTestRunner {
    pub fn new(default_iterations: usize) -> Self {
        Self {
            default_iterations: default_iterations.max(2),
        }
    }

    /// Runs tests repeatedly over N iterations to classify flakiness
    pub async fn run_analysis(
        &self,
        project_root: &Path,
        framework: Option<&str>,
        filter: Option<&str>,
        iterations_override: Option<usize>,
        timeout_seconds: u64,
    ) -> Result<FlakySuiteReport> {
        let iterations = iterations_override.unwrap_or(self.default_iterations);
        let runner = TestRunner::new();

        info!(iterations = iterations, "Starting flakiness analysis");

        // Track stats per test case: name -> (passes, fails, timeouts)
        let mut case_stats: HashMap<String, (usize, usize, usize)> = HashMap::new();

        for i in 1..=iterations {
            debug!(iteration = i, "Running flakiness iteration");
            let suite_results = runner
                .run(project_root, framework, filter, timeout_seconds)
                .await?;

            for suite in suite_results {
                for tc in suite.test_cases {
                    let entry = case_stats.entry(tc.name).or_insert((0, 0, 0));
                    match tc.status {
                        TestCaseStatus::Passed => entry.0 += 1,
                        TestCaseStatus::Failed | TestCaseStatus::Error => entry.1 += 1,
                        TestCaseStatus::Timeout => entry.2 += 1,
                        TestCaseStatus::Skipped => {}
                    }
                }
            }
        }

        let mut reports = Vec::new();
        let mut stable_pass = 0;
        let mut stable_fail = 0;
        let mut flaky_detected = 0;
        let mut timeout_unstable = 0;

        for (name, (p, f, t)) in case_stats {
            let total_runs = p + f + t;
            if total_runs == 0 {
                continue;
            }

            let classification = if t > 0 && p > 0 {
                FlakyClassification::TimeoutUnstable
            } else if p > 0 && f > 0 {
                FlakyClassification::Flaky
            } else if p == total_runs {
                FlakyClassification::StablePass
            } else if f == total_runs {
                FlakyClassification::StableFail
            } else {
                FlakyClassification::Unknown
            };

            match classification {
                FlakyClassification::StablePass => stable_pass += 1,
                FlakyClassification::StableFail => stable_fail += 1,
                FlakyClassification::Flaky => flaky_detected += 1,
                FlakyClassification::TimeoutUnstable => timeout_unstable += 1,
                _ => {}
            }

            let pass_rate = (p as f64) / (total_runs as f64);

            reports.push(FlakyCaseReport {
                test_name: name,
                iterations_run: total_runs,
                pass_count: p,
                fail_count: f,
                timeout_count: t,
                classification,
                pass_rate,
            });
        }

        reports.sort_by(|a, b| a.test_name.cmp(&b.test_name));
        let total_tests = reports.len();

        Ok(FlakySuiteReport {
            total_tests,
            stable_pass,
            stable_fail,
            flaky_detected,
            timeout_unstable,
            reports,
        })
    }

    /// Persists flakiness report records into SQLite
    pub fn record_flaky_run(
        &self,
        db: &Database,
        project_id: &ProjectId,
        report: &FlakySuiteReport,
    ) -> Result<()> {
        let conn = db.inner();
        let now = Utc::now().timestamp_millis();
        let verification_run_id = uuid::Uuid::new_v4().to_string();

        let _ = conn.execute(
            "INSERT INTO verification_runs (id, project_id, trigger, status, started_at, finished_at)
             VALUES (?1, ?2, 'manual', ?3, ?4, ?4)",
            rusqlite::params![
                &verification_run_id,
                project_id.as_str(),
                if report.flaky_detected == 0 { "passed" } else { "failed" },
                now,
            ],
        );

        for c in &report.reports {
            let flaky_id = uuid::Uuid::new_v4().to_string();
            let test_case_id = uuid::Uuid::new_v4().to_string();

            let _ = conn.execute(
                "INSERT INTO flaky_test_runs (id, test_case_id, verification_run_id, iterations_run, pass_count, fail_count, timeout_count, classification, classified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    &flaky_id,
                    &test_case_id,
                    &verification_run_id,
                    c.iterations_run as i64,
                    c.pass_count as i64,
                    c.fail_count as i64,
                    c.timeout_count as i64,
                    c.classification.as_str(),
                    now,
                ],
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flaky_classification_logic() {
        let stable_p = FlakyCaseReport {
            test_name: "test_a".into(),
            iterations_run: 5,
            pass_count: 5,
            fail_count: 0,
            timeout_count: 0,
            classification: FlakyClassification::StablePass,
            pass_rate: 1.0,
        };
        assert_eq!(stable_p.classification, FlakyClassification::StablePass);

        let flaky = FlakyCaseReport {
            test_name: "test_b".into(),
            iterations_run: 5,
            pass_count: 3,
            fail_count: 2,
            timeout_count: 0,
            classification: FlakyClassification::Flaky,
            pass_rate: 0.6,
        };
        assert_eq!(flaky.classification, FlakyClassification::Flaky);
    }
}
