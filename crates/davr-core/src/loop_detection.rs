use davr_config::LoopDetectionConfig;
use davr_storage::Database;
use davr_types::{Result, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopHeuristic {
    RepeatedIdenticalCommands,
    RepeatedFailures,
    FileCycleBack,
    UnchangedState,
    ExcessiveRetries,
}

impl LoopHeuristic {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RepeatedIdenticalCommands => "repeated_identical_commands",
            Self::RepeatedFailures => "repeated_failures",
            Self::FileCycleBack => "file_cycle_back",
            Self::UnchangedState => "unchanged_state",
            Self::ExcessiveRetries => "excessive_retries",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoopNotice {
    pub heuristic: LoopHeuristic,
    pub details: String,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct LoopDetector {
    config: LoopDetectionConfig,
    recent_commands: VecDeque<String>,
    recent_failures: VecDeque<(String, i32)>,
    file_hashes: HashMap<String, Vec<String>>,
    empty_iterations_count: usize,
    consecutive_test_retries: usize,
    file_changes_since_last_test: usize,
    detected_notices: Vec<LoopNotice>,
}

impl LoopDetector {
    pub fn new(config: LoopDetectionConfig) -> Self {
        Self {
            config,
            recent_commands: VecDeque::new(),
            recent_failures: VecDeque::new(),
            file_hashes: HashMap::new(),
            empty_iterations_count: 0,
            consecutive_test_retries: 0,
            file_changes_since_last_test: 0,
            detected_notices: Vec::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn detected_notices(&self) -> &[LoopNotice] {
        &self.detected_notices
    }

    /// Heuristic 1: Repeated Identical Commands
    /// Heuristic 2: Repeated Failures
    pub fn record_command(&mut self, command: &str, exit_code: i32) -> Option<LoopNotice> {
        if !self.config.enabled {
            return None;
        }

        let norm = command.trim().to_string();
        self.recent_commands.push_back(norm.clone());
        if self.recent_commands.len() > self.config.window_iterations {
            self.recent_commands.pop_front();
        }

        // Check Heuristic 2: Consecutive repeated failures with same exit code (prioritized if failing)
        if exit_code != 0 {
            self.recent_failures.push_back((norm.clone(), exit_code));
            if self.recent_failures.len() > self.config.window_iterations {
                self.recent_failures.pop_front();
            }

            // Count trailing identical failures
            let mut consecutive_fail_count = 0;
            for (cmd, code) in self.recent_failures.iter().rev() {
                if cmd == &norm && *code == exit_code {
                    consecutive_fail_count += 1;
                } else {
                    break;
                }
            }

            if consecutive_fail_count >= self.config.repeated_failure_threshold {
                let notice = LoopNotice {
                    heuristic: LoopHeuristic::RepeatedFailures,
                    details: format!(
                        "Command '{}' failed {} consecutive times with exit code {}",
                        norm, consecutive_fail_count, exit_code
                    ),
                    count: consecutive_fail_count,
                };
                self.detected_notices.push(notice.clone());
                return Some(notice);
            }
        }

        // Check Heuristic 1: Count occurrences of this normalized command in sliding window
        let count = self.recent_commands.iter().filter(|&c| c == &norm).count();
        if count >= self.config.repeated_command_threshold {
            let notice = LoopNotice {
                heuristic: LoopHeuristic::RepeatedIdenticalCommands,
                details: format!(
                    "Command '{}' executed {} times within a sliding window of {} commands",
                    norm,
                    count,
                    self.recent_commands.len()
                ),
                count,
            };
            self.detected_notices.push(notice.clone());
            return Some(notice);
        }

        None
    }

    /// Heuristic 3: File Cycle-Back (file content hash returns to previously seen hash)
    pub fn record_file_mutation(
        &mut self,
        file_path: &str,
        content_hash: &str,
    ) -> Option<LoopNotice> {
        if !self.config.enabled {
            return None;
        }

        self.file_changes_since_last_test += 1;
        let history = self.file_hashes.entry(file_path.to_string()).or_default();

        // Check if hash was previously seen at an earlier stage (net-zero work cycle)
        if !history.is_empty()
            && history.contains(&content_hash.to_string())
            && history.last().map(|s| s.as_str()) != Some(content_hash)
        {
            let notice = LoopNotice {
                heuristic: LoopHeuristic::FileCycleBack,
                details: format!(
                    "File '{}' cycled back to a previous content hash (reverted changes)",
                    file_path
                ),
                count: history.len() + 1,
            };
            history.push(content_hash.to_string());
            self.detected_notices.push(notice.clone());
            return Some(notice);
        }

        history.push(content_hash.to_string());
        None
    }

    /// Heuristic 4: Unchanged State Across Iterations
    pub fn record_iteration_state(
        &mut self,
        commands_in_iter: usize,
        fs_events_in_iter: usize,
    ) -> Option<LoopNotice> {
        if !self.config.enabled {
            return None;
        }

        if commands_in_iter == 0 && fs_events_in_iter == 0 {
            self.empty_iterations_count += 1;
            if self.empty_iterations_count >= 3 {
                let notice = LoopNotice {
                    heuristic: LoopHeuristic::UnchangedState,
                    details: format!(
                        "Unchanged project state across {} consecutive iterations (0 commands, 0 file mutations)",
                        self.empty_iterations_count
                    ),
                    count: self.empty_iterations_count,
                };
                self.detected_notices.push(notice.clone());
                return Some(notice);
            }
        } else {
            self.empty_iterations_count = 0;
        }

        None
    }

    /// Heuristic 5: Excessive Retries Without Code Edits
    pub fn record_test_execution(&mut self) -> Option<LoopNotice> {
        if !self.config.enabled {
            return None;
        }

        if self.file_changes_since_last_test == 0 {
            self.consecutive_test_retries += 1;
            if self.consecutive_test_retries >= self.config.excessive_retries_threshold {
                let notice = LoopNotice {
                    heuristic: LoopHeuristic::ExcessiveRetries,
                    details: format!(
                        "Verification / test suite re-run {} consecutive times without any intervening file changes",
                        self.consecutive_test_retries
                    ),
                    count: self.consecutive_test_retries,
                };
                self.detected_notices.push(notice.clone());
                return Some(notice);
            }
        } else {
            self.consecutive_test_retries = 1;
            self.file_changes_since_last_test = 0;
        }

        None
    }

    /// Inspects an existing session stored in SQLite for all 5 loop heuristics
    pub fn inspect_session(
        &mut self,
        db: &Database,
        session_id: &SessionId,
    ) -> Result<Vec<LoopNotice>> {
        let conn = db.inner();
        let mut notices = Vec::new();

        // 1. Analyze commands in sequence
        let mut stmt = conn.prepare(
            "SELECT raw_command, exit_code FROM commands WHERE session_id = ?1 ORDER BY started_at ASC",
        ).map_err(|e| davr_types::DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![session_id.as_str()], |row| {
                let cmd: String = row.get(0)?;
                let exit_code: Option<i32> = row.get(1)?;
                Ok((cmd, exit_code.unwrap_or(0)))
            })
            .map_err(|e| davr_types::DavrError::Database(e.to_string()))?;

        for (cmd, code) in rows.flatten() {
            if let Some(notice) = self.record_command(&cmd, code) {
                notices.push(notice);
            }
        }

        // 2. Analyze filesystem events for cycle-back
        let mut stmt_fs = conn
            .prepare(
                "SELECT file_path, content_hash_after FROM filesystem_events
             WHERE session_id = ?1 AND content_hash_after IS NOT NULL
             ORDER BY detected_at ASC",
            )
            .map_err(|e| davr_types::DavrError::Database(e.to_string()))?;

        let fs_rows = stmt_fs
            .query_map(rusqlite::params![session_id.as_str()], |row| {
                let path: String = row.get(0)?;
                let hash: Option<String> = row.get(1)?;
                Ok((path, hash))
            })
            .map_err(|e| davr_types::DavrError::Database(e.to_string()))?;

        for (path, hash_opt) in fs_rows.flatten() {
            if let Some(hash) = hash_opt {
                if let Some(notice) = self.record_file_mutation(&path, &hash) {
                    notices.push(notice);
                }
            }
        }

        Ok(notices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_repeated_identical_commands() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());
        assert!(detector.record_command("npm test", 0).is_none());
        assert!(detector.record_command("cargo check", 0).is_none());
        assert!(detector.record_command("npm test", 0).is_none());

        let notice = detector.record_command("npm test", 0);
        assert!(notice.is_some());
        let n = notice.unwrap();
        assert_eq!(n.heuristic, LoopHeuristic::RepeatedIdenticalCommands);
        assert_eq!(n.count, 3);
    }

    #[test]
    fn test_heuristic_repeated_failures() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());
        assert!(detector.record_command("pytest", 1).is_none());
        assert!(detector.record_command("pytest", 1).is_none());

        let notice = detector.record_command("pytest", 1);
        assert!(notice.is_some());
        let n = notice.unwrap();
        assert_eq!(n.heuristic, LoopHeuristic::RepeatedFailures);
        assert_eq!(n.count, 3);
    }

    #[test]
    fn test_heuristic_file_cycle_back() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());
        assert!(detector
            .record_file_mutation("src/lib.rs", "hash_v1")
            .is_none());
        assert!(detector
            .record_file_mutation("src/lib.rs", "hash_v2")
            .is_none());

        // Cycle back to hash_v1
        let notice = detector.record_file_mutation("src/lib.rs", "hash_v1");
        assert!(notice.is_some());
        let n = notice.unwrap();
        assert_eq!(n.heuristic, LoopHeuristic::FileCycleBack);
    }

    #[test]
    fn test_heuristic_unchanged_state() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());
        assert!(detector.record_iteration_state(0, 0).is_none());
        assert!(detector.record_iteration_state(0, 0).is_none());

        let notice = detector.record_iteration_state(0, 0);
        assert!(notice.is_some());
        let n = notice.unwrap();
        assert_eq!(n.heuristic, LoopHeuristic::UnchangedState);
    }

    #[test]
    fn test_heuristic_excessive_retries() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());
        detector.record_file_mutation("main.rs", "hash1");
        assert!(detector.record_test_execution().is_none()); // 1st test run after edit
        assert!(detector.record_test_execution().is_none()); // 2nd test run without edit

        let notice = detector.record_test_execution(); // 3rd test run without edit
        assert!(notice.is_some());
        let n = notice.unwrap();
        assert_eq!(n.heuristic, LoopHeuristic::ExcessiveRetries);
    }
}
