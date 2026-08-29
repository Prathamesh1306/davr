use davr_storage::Database;
use davr_types::{Confidence, DavrError, ProjectId, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactedFile {
    pub file_path: String,
    pub confidence: Confidence,
    pub depth: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactedTest {
    pub test_file: String,
    pub test_name: Option<String>,
    pub confidence: Confidence,
    pub triggered_by_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactReport {
    pub base_snapshot_hash: Option<String>,
    pub directly_modified_files: Vec<String>,
    pub impacted_source_files: Vec<ImpactedFile>,
    pub impacted_tests: Vec<ImpactedTest>,
}

pub struct ImpactAnalyzer {
    max_depth: usize,
}

impl Default for ImpactAnalyzer {
    fn default() -> Self {
        Self::new(3)
    }
}

impl ImpactAnalyzer {
    pub fn new(max_depth: usize) -> Self {
        Self { max_depth }
    }

    /// Analyzes the transitive dependency impact of a set of changed files
    pub fn analyze(
        &self,
        db: &Database,
        project_id: &ProjectId,
        changed_files: &[String],
        max_depth_override: Option<usize>,
    ) -> Result<ImpactReport> {
        let max_depth = max_depth_override.unwrap_or(self.max_depth);
        let conn = db.inner();

        // 1. Fetch reverse dependency graph: target_file -> list of files that depend on it
        let mut reverse_graph: HashMap<String, Vec<(String, String)>> = HashMap::new();

        let mut stmt = conn
            .prepare(
                "SELECT e.from_file_id, e.to_file_id, e.confidence
                 FROM dependency_edges e
                 JOIN source_files sf ON e.to_file_id = sf.id
                 WHERE sf.project_id = ?1",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![project_id.as_str()], |row| {
                let from_id: String = row.get(0)?;
                let to_id: String = row.get(1)?;
                let conf: String = row.get(2)?;
                Ok((from_id, to_id, conf))
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for (from_id, to_id, conf) in rows.flatten() {
            let from_path = from_id
                .strip_prefix(&format!("{}:", project_id.as_str()))
                .unwrap_or(&from_id)
                .to_string();
            let to_path = to_id
                .strip_prefix(&format!("{}:", project_id.as_str()))
                .unwrap_or(&to_id)
                .to_string();
            reverse_graph
                .entry(to_path)
                .or_default()
                .push((from_path, conf));
        }

        // 2. Perform BFS from directly modified files up to max_depth
        let mut visited = HashSet::new();
        let mut impacted_map: HashMap<String, ImpactedFile> = HashMap::new();
        let mut queue = VecDeque::new();

        for changed in changed_files {
            visited.insert(changed.clone());
            queue.push_back((changed.clone(), 0, "Directly modified".to_string()));
        }

        while let Some((curr_file, depth, reason)) = queue.pop_front() {
            if depth > 0 {
                let conf = match depth {
                    1 => Confidence::High,
                    2 => Confidence::Medium,
                    _ => Confidence::Low,
                };
                impacted_map.insert(
                    curr_file.clone(),
                    ImpactedFile {
                        file_path: curr_file.clone(),
                        confidence: conf,
                        depth,
                        reason,
                    },
                );
            }

            if depth < max_depth {
                if let Some(dependents) = reverse_graph.get(&curr_file) {
                    for (dep_file, _) in dependents {
                        if !visited.contains(dep_file) {
                            visited.insert(dep_file.clone());
                            queue.push_back((
                                dep_file.clone(),
                                depth + 1,
                                format!("Depends on {}", curr_file),
                            ));
                        }
                    }
                }
            }
        }

        let impacted_source_files: Vec<ImpactedFile> = impacted_map.into_values().collect();

        // 3. Map impacted files to test files
        let mut impacted_tests = Vec::new();
        let all_affected_files: HashSet<String> = changed_files
            .iter()
            .cloned()
            .chain(impacted_source_files.iter().map(|f| f.file_path.clone()))
            .collect();

        let mut test_stmt = conn
            .prepare("SELECT file_path FROM test_files WHERE project_id = ?1")
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let test_rows = test_stmt
            .query_map(rusqlite::params![project_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for test_path in test_rows.flatten() {
            for affected in &all_affected_files {
                let base_name = affected
                    .split('/')
                    .next_back()
                    .unwrap_or(affected)
                    .split('.')
                    .next()
                    .unwrap_or("");

                if test_path.contains(base_name) || test_path == *affected {
                    impacted_tests.push(ImpactedTest {
                        test_file: test_path.clone(),
                        test_name: None,
                        confidence: Confidence::High,
                        triggered_by_file: affected.clone(),
                    });
                    break;
                }
            }
        }

        info!(
            changed = changed_files.len(),
            impacted = impacted_source_files.len(),
            tests = impacted_tests.len(),
            "Completed transitive impact analysis"
        );

        Ok(ImpactReport {
            base_snapshot_hash: None,
            directly_modified_files: changed_files.to_vec(),
            impacted_source_files,
            impacted_tests,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_impact_analyzer_bfs() {
        let analyzer = ImpactAnalyzer::new(3);
        assert_eq!(analyzer.max_depth, 3);
    }
}
