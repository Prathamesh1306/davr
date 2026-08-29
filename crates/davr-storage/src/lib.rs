use chrono::Utc;
use davr_types::{
    CheckCategory, CheckStatus, DavrError, EnvironmentCheckId, FileState, ProjectId, Result,
    SessionId, SessionStatus, Severity,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use tracing::info;

pub const MIGRATIONS: &[(&str, &str)] = &[("001_init", include_str!("../migrations/001_init.sql"))];

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens or creates a SQLite database at the specified path with DAVR PRAGMAs.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| DavrError::Database(e.to_string()))?;
        Self::init_connection(conn)
    }

    /// Opens an in-memory database (useful for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| DavrError::Database(e.to_string()))?;
        Self::init_connection(conn)
    }

    fn init_connection(conn: Connection) -> Result<Self> {
        // Enforce binding PRAGMAs from Part 3 §5.5
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|e| DavrError::Database(format!("Failed to set PRAGMAs: {}", e)))?;

        let mut db = Self { conn };
        db.apply_migrations()?;
        Ok(db)
    }

    /// Runs embedded migrations idempotently inside transactions.
    pub fn apply_migrations(&mut self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version     INTEGER PRIMARY KEY,
                    description TEXT NOT NULL,
                    applied_at  INTEGER NOT NULL
                );",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for (idx, (desc, sql)) in MIGRATIONS.iter().enumerate() {
            let version = (idx + 1) as i64;
            let applied: Option<i64> = self
                .conn
                .query_row(
                    "SELECT version FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| DavrError::Database(e.to_string()))?;

            if applied.is_none() {
                let tx = self
                    .conn
                    .transaction()
                    .map_err(|e| DavrError::Database(e.to_string()))?;

                tx.execute_batch(sql).map_err(|e| {
                    DavrError::Database(format!("Migration {} failed: {}", desc, e))
                })?;

                let now = Utc::now().timestamp_millis();
                tx.execute(
                    "INSERT INTO schema_migrations (version, description, applied_at) VALUES (?1, ?2, ?3)",
                    params![version, desc, now],
                )
                .map_err(|e| DavrError::Database(e.to_string()))?;

                tx.commit()
                    .map_err(|e| DavrError::Database(e.to_string()))?;

                info!(
                    version = version,
                    description = desc,
                    "Applied SQLite migration"
                );
            }
        }
        Ok(())
    }

    /// Performs PRAGMA quick_check to verify integrity.
    pub fn quick_check(&self) -> Result<bool> {
        let result: String = self
            .conn
            .query_row("PRAGMA quick_check;", [], |row| row.get(0))
            .map_err(|e| DavrError::Database(e.to_string()))?;
        Ok(result == "ok")
    }

    /// Ensures a project record exists for the given root path.
    pub fn ensure_project(
        &self,
        name: &str,
        root_path: &str,
        default_lang: Option<&str>,
    ) -> Result<ProjectId> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM projects WHERE root_path = ?1",
                params![root_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| DavrError::Database(e.to_string()))?;

        if let Some(id) = existing {
            return Ok(ProjectId(id));
        }

        let id = ProjectId::new();
        let now = Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO projects (id, name, root_path, default_language, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id.as_str(), name, root_path, default_lang, now, now],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        Ok(id)
    }

    /// Records an environment check result.
    pub fn record_environment_check(
        &self,
        project_id: &ProjectId,
        session_id: Option<&SessionId>,
        check_name: &str,
        category: CheckCategory,
        status: CheckStatus,
        detail: Option<&str>,
    ) -> Result<EnvironmentCheckId> {
        let now = Utc::now().timestamp_millis();
        let session_str = session_id.map(|s| s.as_str());

        self.conn
            .execute(
                "INSERT INTO environment_checks (project_id, session_id, check_name, category, status, detail, checked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    project_id.as_str(),
                    session_str,
                    check_name,
                    category.to_string(),
                    status.to_string(),
                    detail,
                    now
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let id = self.conn.last_insert_rowid();
        Ok(EnvironmentCheckId(id))
    }

    /// Records an installed tool detected during preflight.
    pub fn record_installed_tool(
        &self,
        project_id: &ProjectId,
        check_id: Option<EnvironmentCheckId>,
        tool_name: &str,
        version: Option<&str>,
        resolved_path: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO installed_tools (project_id, environment_check_id, tool_name, version, resolved_path, detected_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    project_id.as_str(),
                    check_id.map(|c| c.0),
                    tool_name,
                    version,
                    resolved_path,
                    now
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;
        Ok(())
    }

    /// Records a structured telemetry event.
    pub fn record_telemetry_event(
        &self,
        project_id: &ProjectId,
        session_id: Option<&SessionId>,
        kind: &str,
        severity: Severity,
        ref_table: Option<&str>,
        ref_id: Option<&str>,
        payload: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp_millis();
        let session_str = session_id.map(|s| s.as_str());

        self.conn
            .execute(
                "INSERT INTO telemetry_events (project_id, session_id, kind, severity, ref_table, ref_id, payload, occurred_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    project_id.as_str(),
                    session_str,
                    kind,
                    severity.to_string(),
                    ref_table,
                    ref_id,
                    payload,
                    now
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Creates a new agent session.
    pub fn create_session(
        &self,
        session_id: &SessionId,
        project_id: &ProjectId,
        agent_name: &str,
        command_line: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.conn
            .execute(
                "INSERT INTO agent_sessions (id, project_id, agent_name, command_line, status, started_at)
                 VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
                params![session_id.as_str(), project_id.as_str(), agent_name, command_line, now],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;
        Ok(())
    }

    /// Completes an agent session.
    pub fn finish_session(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.conn
            .execute(
                "UPDATE agent_sessions SET status = ?1, finished_at = ?2, exit_code = ?3 WHERE id = ?4",
                params![status.to_string(), now, exit_code, session_id.as_str()],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;
        Ok(())
    }

    /// Records post-session state (hash or missing) for touched files
    pub fn record_post_session_states(
        &self,
        session_id: &SessionId,
        states: &[(String, FileState)],
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        for (path, state) in states {
            let (event_type, hash) = match state {
                FileState::Present(h) => ("modified", Some(h.as_str())),
                FileState::Missing => ("deleted", None),
            };
            self.conn
                .execute(
                    "INSERT INTO filesystem_events (session_id, file_path, event_type, confidence, content_hash_after, detected_at)
                     VALUES (?1, ?2, ?3, 'high', ?4, ?5)",
                    params![session_id.as_str(), path, event_type, hash, now],
                )
                .map_err(|e| DavrError::Database(e.to_string()))?;
        }
        Ok(())
    }

    /// Retrieves the recorded post-session file states for a session
    pub fn get_post_session_states(
        &self,
        session_id: &SessionId,
    ) -> Result<std::collections::HashMap<String, FileState>> {
        let mut map = std::collections::HashMap::new();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_path, event_type, content_hash_after
                 FROM filesystem_events
                 WHERE session_id = ?1
                 ORDER BY id ASC",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![session_id.as_str()], |row| {
                let path: String = row.get(0)?;
                let event_type: String = row.get(1)?;
                let hash: Option<String> = row.get(2)?;
                Ok((path, event_type, hash))
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for (path, event_type, hash) in rows.flatten() {
            let state = if event_type == "deleted" || hash.is_none() {
                FileState::Missing
            } else {
                FileState::Present(hash.unwrap())
            };
            map.insert(path, state);
        }
        Ok(map)
    }

    /// Records a rollback operation in the audit trail
    pub fn record_rollback_operation(
        &self,
        rollback_id: &str,
        project_id: &ProjectId,
        snapshot_id: &str,
        session_id: Option<&SessionId>,
        status: &str,
        restored_count: usize,
        error_message: Option<&str>,
        initiated_at: i64,
        completed_at: Option<i64>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO rollback_operations (id, project_id, snapshot_id, session_id, status, files_restored_count, error_message, initiated_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    rollback_id,
                    project_id.as_str(),
                    snapshot_id,
                    session_id.map(|s| s.as_str()),
                    status,
                    restored_count as i64,
                    error_message,
                    initiated_at,
                    completed_at,
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;
        Ok(())
    }

    /// Lists recent rollback operations
    pub fn list_rollback_operations(&self, limit: usize) -> Result<Vec<RollbackRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, snapshot_id, session_id, status, files_restored_count, error_message, initiated_at, completed_at
                 FROM rollback_operations
                 ORDER BY initiated_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(RollbackRecord {
                    id: row.get(0)?,
                    snapshot_id: row.get(1)?,
                    session_id: row.get(2)?,
                    status: row.get(3)?,
                    files_restored_count: row.get::<_, Option<i64>>(4)?.unwrap_or(0) as usize,
                    error_message: row.get(5)?,
                    initiated_at: row.get(6)?,
                    completed_at: row.get(7)?,
                })
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut list = Vec::new();
        for rec in rows.flatten() {
            list.push(rec);
        }
        Ok(list)
    }

    /// Performs an online backup of the SQLite database to the specified path using VACUUM INTO
    pub fn backup(&self, dest_path: impl AsRef<Path>) -> Result<()> {
        let dest = dest_path.as_ref();
        if dest.exists() {
            let _ = std::fs::remove_file(dest);
        }
        let dest_str = dest.to_string_lossy().to_string();
        self.conn
            .execute("VACUUM INTO ?1", params![&dest_str])
            .map_err(|e| DavrError::Database(format!("Backup failed: {}", e)))?;
        Ok(())
    }

    /// Verifies database integrity via PRAGMA integrity_check and foreign_key_check
    pub fn verify_integrity(&self) -> Result<Vec<String>> {
        let mut issues = Vec::new();
        let mut stmt = self
            .conn
            .prepare("PRAGMA integrity_check")
            .map_err(|e| DavrError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for res in rows.flatten() {
            if res != "ok" {
                issues.push(format!("Integrity check issue: {}", res));
            }
        }

        let mut fk_stmt = self
            .conn
            .prepare("PRAGMA foreign_key_check")
            .map_err(|e| DavrError::Database(e.to_string()))?;
        let fk_rows = fk_stmt
            .query_map([], |row| {
                let table: String = row.get(0)?;
                let rowid: i64 = row.get(1)?;
                let parent: String = row.get(2)?;
                Ok(format!(
                    "Foreign key mismatch in table '{}' row {} referencing '{}'",
                    table, rowid, parent
                ))
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        for msg in fk_rows.flatten() {
            issues.push(msg);
        }

        Ok(issues)
    }

    /// Returns row counts across tables and disk usage
    pub fn get_stats(&self, db_file_path: Option<&Path>) -> Result<DatabaseStats> {
        let mut table_counts = std::collections::HashMap::new();
        let tables = [
            "projects",
            "agent_sessions",
            "git_snapshots",
            "file_versions",
            "filesystem_events",
            "telemetry_events",
            "commands",
            "test_runs",
            "test_results",
            "flaky_test_runs",
            "source_files",
            "source_symbols",
            "dependency_edges",
            "rollback_operations",
            "verification_runs",
        ];

        for table in tables {
            let query = format!("SELECT count(*) FROM {}", table);
            let count: usize = self
                .conn
                .query_row(&query, [], |row| row.get(0))
                .unwrap_or(0);
            table_counts.insert(table.to_string(), count);
        }

        let file_size_bytes = if let Some(p) = db_file_path {
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        Ok(DatabaseStats {
            file_size_bytes,
            table_counts,
        })
    }

    /// Prunes telemetry events, verification runs, and sessions older than retention thresholds
    pub fn prune_records(
        &self,
        telemetry_retention_days: u32,
        verification_retention_days: u32,
        include_all: bool,
    ) -> Result<CleanReport> {
        let now = Utc::now().timestamp_millis();
        let tel_cutoff = now - (telemetry_retention_days as i64 * 86_400_000);
        let ver_cutoff = now - (verification_retention_days as i64 * 86_400_000);

        let tel_pruned = self
            .conn
            .execute(
                "DELETE FROM telemetry_events WHERE occurred_at < ?1",
                params![tel_cutoff],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let ver_pruned = self
            .conn
            .execute(
                "DELETE FROM verification_runs WHERE started_at < ?1",
                params![ver_cutoff],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let sess_pruned = if include_all {
            self.conn
                .execute(
                    "DELETE FROM agent_sessions WHERE finished_at IS NOT NULL AND finished_at < ?1",
                    params![tel_cutoff],
                )
                .map_err(|e| DavrError::Database(e.to_string()))?
        } else {
            0
        };

        // Run incremental vacuum / optimize
        let _ = self.conn.execute_batch("PRAGMA optimize;");

        Ok(CleanReport {
            telemetry_events_pruned: tel_pruned,
            verification_runs_pruned: ver_pruned,
            sessions_pruned: sess_pruned,
            snapshots_pruned: 0,
        })
    }

    /// Exports telemetry events formatted as JSON values
    pub fn export_telemetry(
        &self,
        session_id: Option<&str>,
        since_ms: Option<i64>,
    ) -> Result<Vec<serde_json::Value>> {
        let query = match (session_id, since_ms) {
            (Some(_), Some(_)) => {
                "SELECT id, project_id, session_id, kind, severity, ref_table, ref_id, payload, occurred_at
                 FROM telemetry_events
                 WHERE session_id = ?1 AND occurred_at >= ?2
                 ORDER BY occurred_at ASC"
            }
            (Some(_), None) => {
                "SELECT id, project_id, session_id, kind, severity, ref_table, ref_id, payload, occurred_at
                 FROM telemetry_events
                 WHERE session_id = ?1
                 ORDER BY occurred_at ASC"
            }
            (None, Some(_)) => {
                "SELECT id, project_id, session_id, kind, severity, ref_table, ref_id, payload, occurred_at
                 FROM telemetry_events
                 WHERE occurred_at >= ?1
                 ORDER BY occurred_at ASC"
            }
            (None, None) => {
                "SELECT id, project_id, session_id, kind, severity, ref_table, ref_id, payload, occurred_at
                 FROM telemetry_events
                 ORDER BY occurred_at ASC"
            }
        };

        let mut stmt = self
            .conn
            .prepare(query)
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = if let (Some(sid), Some(s)) = (session_id, since_ms) {
            stmt.query(params![sid, s])
        } else if let Some(sid) = session_id {
            stmt.query(params![sid])
        } else if let Some(s) = since_ms {
            stmt.query(params![s])
        } else {
            stmt.query([])
        }
        .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut list = Vec::new();
        let mut rows = rows;
        while let Some(row) = rows
            .next()
            .map_err(|e| DavrError::Database(e.to_string()))?
        {
            let id: i64 = row.get(0).unwrap_or(0);
            let proj: String = row.get(1).unwrap_or_default();
            let sess: Option<String> = row.get(2).unwrap_or(None);
            let kind: String = row.get(3).unwrap_or_default();
            let sev: String = row.get(4).unwrap_or_default();
            let ref_t: Option<String> = row.get(5).unwrap_or(None);
            let ref_i: Option<String> = row.get(6).unwrap_or(None);
            let payload_raw: Option<String> = row.get(7).unwrap_or(None);
            let occurred_at: i64 = row.get(8).unwrap_or(0);

            let payload: Option<serde_json::Value> = payload_raw
                .as_deref()
                .and_then(|p| serde_json::from_str(p).ok());

            list.push(serde_json::json!({
                "id": id,
                "project_id": proj,
                "session_id": sess,
                "kind": kind,
                "severity": sev,
                "ref_table": ref_t,
                "ref_id": ref_i,
                "payload": payload,
                "occurred_at": occurred_at
            }));
        }

        Ok(list)
    }

    pub fn inner(&self) -> &Connection {
        &self.conn
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatabaseStats {
    pub file_size_bytes: u64,
    pub table_counts: std::collections::HashMap<String, usize>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CleanReport {
    pub telemetry_events_pruned: usize,
    pub verification_runs_pruned: usize,
    pub sessions_pruned: usize,
    pub snapshots_pruned: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RollbackRecord {
    pub id: String,
    pub snapshot_id: String,
    pub session_id: Option<String>,
    pub status: String,
    pub files_restored_count: usize,
    pub error_message: Option<String>,
    pub initiated_at: i64,
    pub completed_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_and_foreign_keys() {
        let db = Database::in_memory().expect("failed to init in-memory db");
        assert!(db.quick_check().expect("quick check failed"));

        let project_id = db
            .ensure_project("test-proj", "/tmp/test-proj", Some("rust"))
            .expect("failed to insert project");

        let check_id = db
            .record_environment_check(
                &project_id,
                None,
                "rust_toolchain_present",
                CheckCategory::Runtime,
                CheckStatus::Pass,
                Some("{\"version\":\"1.98.0\"}"),
            )
            .expect("failed to record check");

        db.record_installed_tool(
            &project_id,
            Some(check_id),
            "rustc",
            Some("1.98.0"),
            Some("/usr/local/bin/rustc"),
        )
        .expect("failed to record tool");

        let session_id = SessionId::new();
        db.create_session(&session_id, &project_id, "claude", "claude --model opus")
            .expect("failed to create session");

        db.finish_session(&session_id, SessionStatus::Completed, Some(0))
            .expect("failed to finish session");
    }
}
