use chrono::Utc;
use davr_agent::ProcessSupervisor;
use davr_config::Config;
use davr_env::{CheckResult, EnvironmentValidator};
use davr_fs::FilesystemMonitor;
use davr_git::{FileDiffSummary, GitManager, RollbackReport, RollbackScope};
use davr_storage::Database;
use davr_telemetry::TelemetryEmitter;
use davr_types::{
    CheckCategory, CheckStatus, DavrError, Result, SessionId, SessionStatus, Severity,
    SnapshotReason,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

pub use davr_flaky::{FlakyCaseReport, FlakyClassification, FlakySuiteReport};
pub use davr_test::{TestCaseResult, TestCaseStatus, TestSuiteResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub agent_name: String,
    pub status: String,
    pub exit_code: i32,
    pub pre_snapshot_id: Option<String>,
    pub files_changed: Vec<String>,
    pub commands_run: usize,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListItem {
    pub id: String,
    pub agent_name: String,
    pub command_line: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub id: String,
    pub agent_name: String,
    pub command_line: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub touched_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceItem {
    pub kind: String,
    pub severity: String,
    pub occurred_at: i64,
    pub ref_table: Option<String>,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotListItem {
    pub id: String,
    pub tree_hash: String,
    pub reason: String,
    pub dirty_before: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDetail {
    pub id: String,
    pub tree_hash: String,
    pub reason: String,
    pub dirty_before: bool,
    pub created_at: i64,
    pub session_id: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentHealthSummary {
    pub total_checks: usize,
    pub passed: usize,
    pub warnings: usize,
    pub failures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSummary {
    pub project_name: String,
    pub project_root: String,
    pub languages: Vec<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub last_session: Option<SessionDetail>,
    pub last_snapshot: Option<SnapshotListItem>,
    pub environment_health: EnvironmentHealthSummary,
}

pub struct CoreEngine {
    project_root: PathBuf,
}

impl CoreEngine {
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Initializes .davr/ directory, config.toml, and migrates davr.db
    pub fn init(&self, force: bool, languages: Option<Vec<String>>) -> Result<PathBuf> {
        let davr_dir = self.project_root.join(".davr");
        if davr_dir.exists() && !force {
            return Err(DavrError::General(
                "DAVR is already initialized. Use --force to reinitialize.".into(),
            ));
        }

        fs::create_dir_all(&davr_dir)
            .map_err(|e| DavrError::General(format!("Failed to create .davr dir: {}", e)))?;

        let env_validator = EnvironmentValidator::new();
        let detected_langs =
            languages.unwrap_or_else(|| env_validator.detect_languages(&self.project_root));

        let mut config = Config::default();
        let project_name = self
            .project_root
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "my-project".into());

        config.project.name = project_name;
        config.project.languages = detected_langs;

        let config_path = davr_dir.join("config.toml");
        let toml_str = config.to_toml_string()?;
        fs::write(&config_path, toml_str)
            .map_err(|e| DavrError::General(format!("Failed to write config.toml: {}", e)))?;

        let db_path = davr_dir.join("davr.db");
        let db = Database::open(&db_path)?;
        let canonical_root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());

        db.ensure_project(
            &config.project.name,
            &canonical_root.to_string_lossy(),
            config.project.languages.first().map(|s| s.as_str()),
        )?;

        info!(path = %config_path.display(), "Successfully initialized DAVR project");
        Ok(config_path)
    }

    /// Runs pre-flight environment checks
    pub fn doctor(&self, categories: Option<Vec<CheckCategory>>) -> Result<Vec<CheckResult>> {
        let config = Config::load_from_dir(&self.project_root)?;
        let validator = EnvironmentValidator::new();
        let mut results = validator.validate(&self.project_root, &config);

        if let Some(cats) = categories {
            results.retain(|r| cats.contains(&r.category));
        }

        let db_path = self.project_root.join(".davr").join("davr.db");
        if db_path.exists() {
            if let Ok(db) = Database::open(&db_path) {
                let canonical_root = self
                    .project_root
                    .canonicalize()
                    .unwrap_or_else(|_| self.project_root.clone());

                if let Ok(project_id) = db.ensure_project(
                    &config.project.name,
                    &canonical_root.to_string_lossy(),
                    config.project.languages.first().map(|s| s.as_str()),
                ) {
                    for check in &results {
                        let detail_json = serde_json::to_string(&check).unwrap_or_default();
                        let check_id = db
                            .record_environment_check(
                                &project_id,
                                None,
                                &check.name,
                                check.category,
                                check.status,
                                Some(&detail_json),
                            )
                            .ok();

                        if let Some(tool_name) = &check.tool_name {
                            let _ = db.record_installed_tool(
                                &project_id,
                                check_id,
                                tool_name,
                                check.tool_version.as_deref(),
                                check.resolved_path.as_deref(),
                            );
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Orchestrates a fully supervised agent session
    pub async fn run_agent_session(
        &self,
        agent_override: Option<&str>,
        command: &str,
        args: &[String],
        no_snapshot: bool,
    ) -> Result<SessionSummary> {
        let start_time = Utc::now().timestamp_millis();
        let config = Config::load_from_dir(&self.project_root)?;

        // 1. Pre-flight checks
        let doctor_results = self.doctor(None)?;
        let has_failures = doctor_results.iter().any(|r| r.status == CheckStatus::Fail);
        if has_failures {
            return Err(DavrError::Environment(
                "Pre-flight environment checks failed. Run `davr doctor` to inspect.".into(),
            ));
        }

        // 2. Open DB & Register Session
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            let _ = self.init(false, None)?;
        }
        let db = Database::open(&db_path)?;
        let canonical_root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());
        let project_id = db.ensure_project(
            &config.project.name,
            &canonical_root.to_string_lossy(),
            config.project.languages.first().map(|s| s.as_str()),
        )?;

        let session_id = SessionId::new();
        let agent_name = agent_override.unwrap_or(&config.agent.default_agent);
        let raw_command_line = format!("{} {}", command, args.join(" "));

        let db_arc = Arc::new(Mutex::new(db));
        let telemetry = TelemetryEmitter::new(
            Arc::clone(&db_arc),
            project_id.clone(),
            Some(session_id.clone()),
            config.telemetry.enabled,
        );

        // Top-level Security Policy Guard & Secret Redaction
        let security = davr_security::SecurityEngine::from_config(&config.security)?;
        let full_command_line = security.redact_secrets(&raw_command_line);

        let decision = security.evaluate_command(&raw_command_line);
        if decision == davr_types::PolicyDecision::Blocked {
            let _ = telemetry.emit(
                "COMMAND_BLOCKED",
                Severity::Error,
                None,
                None,
                Some(serde_json::json!({
                    "command": raw_command_line,
                    "reason": "Blocked by security policy pattern"
                })),
            );
            let _ = telemetry.flush();
            return Err(DavrError::Security(format!(
                "Command blocked by security policy: {}",
                raw_command_line
            )));
        }

        {
            let locked_db = db_arc.lock().unwrap();
            locked_db.create_session(&session_id, &project_id, agent_name, &full_command_line)?;
        }

        // 3. Pre-run Git Snapshot
        let git_mgr = GitManager::new(&self.project_root);
        let snapshot_info = if config.git.snapshot_on_run && !no_snapshot && git_mgr.is_git_repo() {
            let locked_db = db_arc.lock().unwrap();
            let snap = git_mgr.create_snapshot(
                Some(&locked_db),
                Some(&project_id),
                Some(&session_id),
                SnapshotReason::PreRun,
            )?;
            let _ = git_mgr.prune_old_snapshots(
                &locked_db,
                &project_id,
                config.git.max_snapshots_per_project,
            );
            telemetry.emit(
                "SNAPSHOT_CREATED",
                Severity::Info,
                Some("git_snapshots"),
                Some(snap.id.as_str()),
                Some(serde_json::json!({ "reason": "pre_run", "tree_hash": snap.tree_hash })),
            )?;
            Some(snap)
        } else {
            None
        };

        // 4. Start Filesystem Watcher
        let mut fs_monitor = FilesystemMonitor::start(&self.project_root).ok();

        // 5. Emit SESSION_STARTED
        telemetry.emit(
            "SESSION_STARTED",
            Severity::Info,
            Some("agent_sessions"),
            Some(session_id.as_str()),
            Some(serde_json::json!({
                "agent": agent_name,
                "command": full_command_line
            })),
        )?;

        // 6. Spawn and supervise agent process
        let supervisor = ProcessSupervisor::new(&self.project_root, config.agent.clone());
        let agent_result = supervisor.run_supervised(command, args).await;

        // 7. Drain and record filesystem events
        let mut touched_set = HashSet::new();
        {
            let locked_db = db_arc.lock().unwrap();
            let conn = locked_db.inner();

            if let Some(ref mut monitor) = fs_monitor {
                let events = monitor.drain_all_events();
                for ev in events {
                    if !ev.path.starts_with(".davr") && touched_set.insert(ev.path.clone()) {
                        let _ = conn.execute(
                            "INSERT INTO filesystem_events (session_id, file_path, event_type, confidence, content_hash_after, detected_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![
                                session_id.as_str(),
                                &ev.path,
                                format!("{:?}", ev.kind).to_lowercase(),
                                ev.confidence.to_string(),
                                ev.content_hash_after.as_deref(),
                                ev.detected_at
                            ],
                        );
                    }
                }
            }

            // Fail-safe: Reconcile with Git snapshot diff so no fast mutation is ever missed
            if let Some(ref snap) = snapshot_info {
                if let Ok(diffs) = git_mgr.diff_snapshot(&snap.tree_hash) {
                    let now = Utc::now().timestamp_millis();
                    for d in diffs {
                        if !d.file_path.starts_with(".davr")
                            && touched_set.insert(d.file_path.clone())
                        {
                            let _ = conn.execute(
                                "INSERT INTO filesystem_events (session_id, file_path, event_type, confidence, content_hash_after, detected_at)
                                 VALUES (?1, ?2, ?3, 'high', NULL, ?4)",
                                rusqlite::params![
                                    session_id.as_str(),
                                    &d.file_path,
                                    &d.change_type,
                                    now
                                ],
                            );
                        }
                    }
                }
            }
        }

        let touched_files: Vec<String> = touched_set.into_iter().collect();

        // 8. Capture Post-Session File States (BLAKE3 or Missing) for deterministic conflict detection
        let mut post_session_states = Vec::new();
        for path_str in &touched_files {
            let full_p = self.project_root.join(path_str);
            if full_p.exists() && !full_p.is_dir() {
                if let Ok(bytes) = fs::read(&full_p) {
                    let h = blake3::hash(&bytes).to_hex().to_string();
                    post_session_states.push((path_str.clone(), davr_types::FileState::Present(h)));
                }
            } else {
                post_session_states.push((path_str.clone(), davr_types::FileState::Missing));
            }
        }
        {
            let locked_db = db_arc.lock().unwrap();
            let _ = locked_db.record_post_session_states(&session_id, &post_session_states);
        }

        let exit_code = match &agent_result {
            Ok(code) => *code,
            Err(_) => 1,
        };

        let session_status = if exit_code == 0 {
            SessionStatus::Completed
        } else {
            SessionStatus::Failed
        };

        // 9. Emit SESSION_FINISHED & flush
        telemetry.emit(
            "SESSION_FINISHED",
            if exit_code == 0 {
                Severity::Info
            } else {
                Severity::Warn
            },
            Some("agent_sessions"),
            Some(session_id.as_str()),
            Some(serde_json::json!({
                "exit_code": exit_code,
                "files_changed_count": touched_files.len()
            })),
        )?;
        telemetry.flush()?;

        {
            let locked_db = db_arc.lock().unwrap();
            locked_db.finish_session(&session_id, session_status, Some(exit_code))?;
        }

        let duration_ms = Utc::now().timestamp_millis() - start_time;

        Ok(SessionSummary {
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            status: session_status.to_string(),
            exit_code,
            pre_snapshot_id: snapshot_info.map(|s| s.tree_hash),
            files_changed: touched_files,
            commands_run: 1,
            duration_ms,
        })
    }

    /// Lists recent sessions
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionListItem>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_name, command_line, status, started_at, finished_at, exit_code
                 FROM agent_sessions ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(SessionListItem {
                    id: row.get(0)?,
                    agent_name: row.get(1)?,
                    command_line: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get(4)?,
                    finished_at: row.get(5)?,
                    exit_code: row.get(6)?,
                })
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut list = Vec::new();
        for item in rows.flatten() {
            list.push(item);
        }
        Ok(list)
    }

    /// Fetches detailed information for a specific session by ID or prefix
    pub fn get_session_detail(&self, session_id: &str) -> Result<SessionDetail> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        let row = conn
            .query_row(
                "SELECT id, agent_name, command_line, status, started_at, finished_at, exit_code
                 FROM agent_sessions
                 WHERE id = ?1 OR id LIKE ?2
                 LIMIT 1",
                rusqlite::params![session_id, format!("{}%", session_id)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i32>>(6)?,
                    ))
                },
            )
            .map_err(|_| DavrError::General(format!("Session not found: {}", session_id)))?;

        let (id, agent_name, command_line, status, started_at, finished_at, exit_code) = row;
        let duration_ms = finished_at.map(|f| f - started_at);

        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT file_path FROM filesystem_events WHERE session_id = ?1 ORDER BY file_path ASC",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let file_rows = stmt
            .query_map(rusqlite::params![&id], |r| r.get::<_, String>(0))
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut touched_files = Vec::new();
        for path in file_rows.flatten() {
            touched_files.push(path);
        }

        Ok(SessionDetail {
            id,
            agent_name,
            command_line,
            status,
            started_at,
            finished_at,
            exit_code,
            duration_ms,
            touched_files,
        })
    }

    /// Executes a single arbitrary command under DAVR policy and telemetry supervision
    pub async fn exec(
        &self,
        command: &str,
        args: &[String],
        session_id_override: Option<&str>,
    ) -> Result<i32> {
        let config = Config::load_from_dir(&self.project_root)?;
        let raw_command_line = if args.is_empty() {
            command.to_string()
        } else {
            format!("{} {}", command, args.join(" "))
        };

        // 1. Security policy check
        let security = davr_security::SecurityEngine::from_config(&config.security)?;
        let decision = security.evaluate_command(&raw_command_line);
        if decision == davr_types::PolicyDecision::Blocked {
            return Err(DavrError::Security(format!(
                "Command blocked by security policy: {}",
                raw_command_line
            )));
        }

        // 2. Open DB & Telemetry if available
        let db_path = self.project_root.join(".davr").join("davr.db");
        let telemetry = if db_path.exists() {
            if let Ok(db) = Database::open(&db_path) {
                let canonical_root = self
                    .project_root
                    .canonicalize()
                    .unwrap_or_else(|_| self.project_root.clone());
                if let Ok(project_id) = db.ensure_project(
                    &config.project.name,
                    &canonical_root.to_string_lossy(),
                    config.project.languages.first().map(|s| s.as_str()),
                ) {
                    let db_arc = Arc::new(Mutex::new(db));
                    let session_obj = session_id_override.map(SessionId::from_string);
                    Some(TelemetryEmitter::new(
                        db_arc,
                        project_id,
                        session_obj,
                        config.telemetry.enabled,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ref tel) = telemetry {
            let redacted = security.redact_secrets(&raw_command_line);
            let _ = tel.emit(
                "COMMAND_STARTED",
                Severity::Info,
                Some("commands"),
                None,
                Some(serde_json::json!({ "command": redacted })),
            );
        }

        // 3. Process execution
        let supervisor = ProcessSupervisor::new(&self.project_root, config.agent.clone());
        let start = Utc::now().timestamp_millis();
        let result = supervisor.run_supervised(command, args).await;
        let duration_ms = Utc::now().timestamp_millis() - start;

        let exit_code = match &result {
            Ok(code) => *code,
            Err(_) => 1,
        };

        if let Some(ref tel) = telemetry {
            let _ = tel.emit(
                if exit_code == 0 {
                    "COMMAND_FINISHED"
                } else {
                    "COMMAND_FAILED"
                },
                if exit_code == 0 {
                    Severity::Info
                } else {
                    Severity::Warn
                },
                Some("commands"),
                None,
                Some(serde_json::json!({
                    "exit_code": exit_code,
                    "duration_ms": duration_ms
                })),
            );
            let _ = tel.flush();
        }

        Ok(exit_code)
    }

    /// Shows high-level project status, last session summary, and environment health
    pub fn status(&self, session_id_filter: Option<&str>) -> Result<StatusSummary> {
        let config = Config::load_from_dir(&self.project_root)?;
        let git_mgr = GitManager::new(&self.project_root);
        let git_dirty = git_mgr.is_dirty().unwrap_or(false);

        let git_branch = git_mgr.current_branch();

        let last_session = if let Some(sid) = session_id_filter {
            self.get_session_detail(sid).ok()
        } else {
            let db_path = self.project_root.join(".davr").join("davr.db");
            if db_path.exists() {
                if let Ok(db) = Database::open(&db_path) {
                    let conn = db.inner();
                    let latest_id: Option<String> = conn
                        .query_row(
                            "SELECT id FROM agent_sessions ORDER BY started_at DESC LIMIT 1",
                            [],
                            |row| row.get(0),
                        )
                        .ok();
                    latest_id.and_then(|id| self.get_session_detail(&id).ok())
                } else {
                    None
                }
            } else {
                None
            }
        };

        let last_snapshot = self
            .list_snapshots()
            .ok()
            .and_then(|snaps| snaps.into_iter().next());

        let doctor_results = self.doctor(None).unwrap_or_default();
        let health = EnvironmentHealthSummary {
            total_checks: doctor_results.len(),
            passed: doctor_results
                .iter()
                .filter(|r| r.status == CheckStatus::Pass)
                .count(),
            warnings: doctor_results
                .iter()
                .filter(|r| r.status == CheckStatus::Warn)
                .count(),
            failures: doctor_results
                .iter()
                .filter(|r| r.status == CheckStatus::Fail)
                .count(),
        };

        Ok(StatusSummary {
            project_name: config.project.name,
            project_root: self.project_root.display().to_string(),
            languages: config.project.languages,
            git_branch,
            git_dirty,
            last_session,
            last_snapshot,
            environment_health: health,
        })
    }

    /// Fetches telemetry trace events
    pub fn get_trace(
        &self,
        session_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<TraceItem>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        let query = match (session_id, kind) {
            (Some(_), Some(_)) => {
                "SELECT kind, severity, occurred_at, ref_table, payload FROM telemetry_events WHERE session_id = ?1 AND kind = ?2 ORDER BY occurred_at ASC"
            }
            (Some(_), None) => {
                "SELECT kind, severity, occurred_at, ref_table, payload FROM telemetry_events WHERE session_id = ?1 ORDER BY occurred_at ASC"
            }
            (None, Some(_)) => {
                "SELECT kind, severity, occurred_at, ref_table, payload FROM telemetry_events WHERE kind = ?1 ORDER BY occurred_at ASC"
            }
            (None, None) => {
                "SELECT kind, severity, occurred_at, ref_table, payload FROM telemetry_events ORDER BY occurred_at ASC LIMIT 100"
            }
        };

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut items = Vec::new();
        let rows = if let (Some(sid), Some(k)) = (session_id, kind) {
            stmt.query(rusqlite::params![sid, k])
        } else if let Some(sid) = session_id {
            stmt.query(rusqlite::params![sid])
        } else if let Some(k) = kind {
            stmt.query(rusqlite::params![k])
        } else {
            stmt.query([])
        }
        .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut rows = rows;
        while let Some(row) = rows
            .next()
            .map_err(|e| DavrError::Database(e.to_string()))?
        {
            items.push(TraceItem {
                kind: row.get(0).unwrap_or_default(),
                severity: row.get(1).unwrap_or_default(),
                occurred_at: row.get(2).unwrap_or_default(),
                ref_table: row.get(3).unwrap_or(None),
                payload: row.get(4).unwrap_or(None),
            });
        }

        Ok(items)
    }

    /// Lists Git snapshots
    pub fn list_snapshots(&self) -> Result<Vec<SnapshotListItem>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        let mut stmt = conn
            .prepare("SELECT id, tree_hash, reason, dirty_before, created_at FROM git_snapshots ORDER BY created_at DESC")
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(SnapshotListItem {
                    id: row.get(0)?,
                    tree_hash: row.get(1)?,
                    reason: row.get(2)?,
                    dirty_before: row.get::<_, i64>(3)? == 1,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut list = Vec::new();
        for item in rows.flatten() {
            list.push(item);
        }
        Ok(list)
    }

    /// Creates a manual Git snapshot of current working tree
    pub fn create_snapshot(&self, reason: Option<&str>) -> Result<SnapshotListItem> {
        let git_mgr = GitManager::new(&self.project_root);
        if !git_mgr.is_git_repo() {
            return Err(DavrError::Git(
                "Target directory is not a Git repository".into(),
            ));
        }

        let config = Config::load_from_dir(&self.project_root)?;
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            let _ = self.init(false, None)?;
        }
        let db = Database::open(&db_path)?;
        let canonical_root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());
        let project_id = db.ensure_project(
            &config.project.name,
            &canonical_root.to_string_lossy(),
            config.project.languages.first().map(|s| s.as_str()),
        )?;

        let snap_reason = if let Some(r) = reason {
            if r.contains("pre_run") {
                SnapshotReason::PreRun
            } else if r.contains("rollback") {
                SnapshotReason::PreRollback
            } else {
                SnapshotReason::Manual
            }
        } else {
            SnapshotReason::Manual
        };

        let snap = git_mgr.create_snapshot(Some(&db), Some(&project_id), None, snap_reason)?;
        let _ = git_mgr.prune_old_snapshots(&db, &project_id, config.git.max_snapshots_per_project);

        Ok(SnapshotListItem {
            id: snap.id.as_str().to_string(),
            tree_hash: snap.tree_hash,
            reason: snap.reason.to_string(),
            dirty_before: snap.dirty_before,
            created_at: snap.created_at,
        })
    }

    /// Fetches details and files for a specific snapshot by ID or tree hash prefix
    pub fn get_snapshot_detail(&self, id_or_hash: &str) -> Result<SnapshotDetail> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        let row = conn
            .query_row(
                "SELECT id, tree_hash, reason, dirty_before, created_at, session_id
                 FROM git_snapshots
                 WHERE id = ?1 OR tree_hash LIKE ?2
                 LIMIT 1",
                rusqlite::params![id_or_hash, format!("{}%", id_or_hash)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? == 1,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .map_err(|_| DavrError::Git(format!("Snapshot not found: {}", id_or_hash)))?;

        let (id, tree_hash, reason, dirty_before, created_at, session_id) = row;

        let mut stmt = conn
            .prepare(
                "SELECT file_path FROM file_versions WHERE snapshot_id = ?1 ORDER BY file_path ASC",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let file_rows = stmt
            .query_map(rusqlite::params![&id], |r| r.get::<_, String>(0))
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut files = Vec::new();
        for path in file_rows.flatten() {
            files.push(path);
        }

        Ok(SnapshotDetail {
            id,
            tree_hash,
            reason,
            dirty_before,
            created_at,
            session_id,
            files,
        })
    }

    /// Performs safe rollback to a prior snapshot
    pub fn rollback(
        &self,
        snapshot_tree_hash: Option<&str>,
        session_id: Option<&str>,
        scope: RollbackScope,
        dry_run: bool,
        force: bool,
    ) -> Result<RollbackReport> {
        let git_mgr = GitManager::new(&self.project_root);
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let conn = db.inner();

        // Resolve snapshot tree hash
        let target_tree_hash = if let Some(hash) = snapshot_tree_hash {
            hash.to_string()
        } else {
            // Find newest snapshot
            conn.query_row(
                "SELECT tree_hash FROM git_snapshots ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| DavrError::Git("No snapshots found to rollback to".into()))?
        };

        let resolved_session_id = if let Some(sid) = session_id {
            Some(sid.to_string())
        } else {
            conn.query_row(
                "SELECT id FROM agent_sessions ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok()
        };

        // Query session touched files and post-session states
        let mut touched = HashSet::new();
        let mut post_session_states = HashMap::new();

        if let Some(ref sid) = resolved_session_id {
            let session_obj = SessionId::from_string(sid.clone());
            if let Ok(states) = db.get_post_session_states(&session_obj) {
                for (f, state) in states {
                    touched.insert(f.clone());
                    post_session_states.insert(f, state);
                }
            }

            let mut stmt = conn
                .prepare("SELECT file_path FROM filesystem_events WHERE session_id = ?1")
                .map_err(|e| DavrError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![sid], |row| row.get::<_, String>(0))
                .map_err(|e| DavrError::Database(e.to_string()))?;
            for f in rows.flatten() {
                touched.insert(f);
            }
        }

        let initiated_at = Utc::now().timestamp_millis();
        let report = git_mgr.rollback(
            &target_tree_hash,
            &touched,
            &post_session_states,
            scope,
            dry_run,
            force,
        )?;

        if !dry_run {
            let config = Config::load_from_dir(&self.project_root)?;
            let canonical_root = self
                .project_root
                .canonicalize()
                .unwrap_or_else(|_| self.project_root.clone());
            let project_id = db.ensure_project(
                &config.project.name,
                &canonical_root.to_string_lossy(),
                config.project.languages.first().map(|s| s.as_str()),
            )?;

            let snapshot_record_id: String = conn
                .query_row(
                    "SELECT id FROM git_snapshots WHERE tree_hash = ?1 ORDER BY created_at DESC LIMIT 1",
                    [&target_tree_hash],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| target_tree_hash.clone());

            let completed_at = Utc::now().timestamp_millis();
            let _ = db.record_rollback_operation(
                report.id.as_str(),
                &project_id,
                &snapshot_record_id,
                resolved_session_id
                    .as_ref()
                    .map(|s| SessionId::from_string(s.clone()))
                    .as_ref(),
                &report.status,
                report.restored_files.len() + report.deleted_files.len(),
                report.error_message.as_deref(),
                initiated_at,
                Some(completed_at),
            );
        }

        Ok(report)
    }

    /// Lists past rollback audit records
    pub fn list_rollbacks(&self, limit: usize) -> Result<Vec<davr_storage::RollbackRecord>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Ok(Vec::new());
        }
        let db = Database::open(&db_path)?;
        db.list_rollback_operations(limit)
    }

    /// Computes file diff for a snapshot
    pub fn diff_snapshot(&self, snapshot_tree_hash: &str) -> Result<Vec<FileDiffSummary>> {
        let git_mgr = GitManager::new(&self.project_root);
        git_mgr.diff_snapshot(snapshot_tree_hash)
    }

    /// Runs tests across detected frameworks and records results
    pub async fn run_tests(
        &self,
        framework_override: Option<&str>,
        filter: Option<&str>,
    ) -> Result<Vec<davr_test::TestSuiteResult>> {
        let config = Config::load_from_dir(&self.project_root)?;
        let runner = davr_test::TestRunner::new();

        let timeout = if config.agent.timeout_seconds > 0 {
            config.agent.timeout_seconds
        } else {
            300 // default 5m timeout
        };

        let db_path = self.project_root.join(".davr").join("davr.db");
        let (db_arc_opt, project_id_opt, telemetry_opt) = if db_path.exists() {
            if let Ok(db) = Database::open(&db_path) {
                let canonical_root = self
                    .project_root
                    .canonicalize()
                    .unwrap_or_else(|_| self.project_root.clone());

                if let Ok(project_id) = db.ensure_project(
                    &config.project.name,
                    &canonical_root.to_string_lossy(),
                    config.project.languages.first().map(|s| s.as_str()),
                ) {
                    let db_arc = Arc::new(Mutex::new(db));
                    let tel = TelemetryEmitter::new(
                        Arc::clone(&db_arc),
                        project_id.clone(),
                        None,
                        config.telemetry.enabled,
                    );
                    (Some(db_arc), Some(project_id), Some(tel))
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

        // Emit VERIFICATION_STARTED
        let start_ms = Utc::now().timestamp_millis();
        if let Some(ref tel) = telemetry_opt {
            let _ = tel.emit(
                "VERIFICATION_STARTED",
                Severity::Info,
                Some("verification_runs"),
                None,
                Some(serde_json::json!({
                    "framework": framework_override,
                    "filter": filter,
                })),
            );
        }

        let results = runner
            .run(&self.project_root, framework_override, filter, timeout)
            .await?;

        let duration_ms = Utc::now().timestamp_millis() - start_ms;
        let total_passed: usize = results.iter().map(|r| r.passed).sum();
        let total_failed: usize = results.iter().map(|r| r.failed).sum();
        let total_skipped: usize = results.iter().map(|r| r.skipped).sum();
        let any_failed = results.iter().any(|r| r.exit_code != 0 || r.failed > 0);

        if let (Some(ref db_arc), Some(ref project_id)) = (&db_arc_opt, &project_id_opt) {
            let locked_db = db_arc.lock().unwrap();
            let _ = runner.record_test_run(&locked_db, project_id, None, &results);
        }

        // Emit VERIFICATION_FINISHED
        if let Some(ref tel) = telemetry_opt {
            let _ = tel.emit(
                "VERIFICATION_FINISHED",
                if any_failed {
                    Severity::Warn
                } else {
                    Severity::Info
                },
                Some("verification_runs"),
                None,
                Some(serde_json::json!({
                    "passed": total_passed,
                    "failed": total_failed,
                    "skipped": total_skipped,
                    "duration_ms": duration_ms,
                    "success": !any_failed,
                })),
            );
            let _ = tel.flush();
        }

        Ok(results)
    }

    /// Indexes project source code symbols and dependency edges
    pub fn analyze_project(&self) -> Result<AnalysisSummary> {
        let config = Config::load_from_dir(&self.project_root)?;
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            let _ = self.init(false, None)?;
        }
        let db = Database::open(&db_path)?;
        let canonical_root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());
        let project_id = db.ensure_project(
            &config.project.name,
            &canonical_root.to_string_lossy(),
            config.project.languages.first().map(|s| s.as_str()),
        )?;

        let ast_engine = davr_ast::AstEngine::new();
        let total_files = ast_engine.index_project(&db, &project_id, &self.project_root)?;

        let conn = db.inner();
        let total_symbols: usize = conn
            .query_row(
                "SELECT count(*) FROM source_symbols ss JOIN source_files sf ON ss.source_file_id = sf.id WHERE sf.project_id = ?1",
                rusqlite::params![project_id.as_str()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let total_edges: usize = conn
            .query_row(
                "SELECT count(*) FROM dependency_edges de JOIN source_files sf ON de.from_file_id = sf.id WHERE sf.project_id = ?1",
                rusqlite::params![project_id.as_str()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(AnalysisSummary {
            files_indexed: total_files,
            symbols_extracted: total_symbols,
            dependency_edges: total_edges,
        })
    }

    /// Computes transitive impact analysis of changes
    pub fn analyze_impact(
        &self,
        snapshot_tree_hash: Option<&str>,
        max_depth: usize,
    ) -> Result<davr_impact::ImpactReport> {
        let _ = self.analyze_project()?; // Ensure AST index is fresh

        let config = Config::load_from_dir(&self.project_root)?;
        let db_path = self.project_root.join(".davr").join("davr.db");
        let db = Database::open(&db_path)?;
        let canonical_root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());
        let project_id = db.ensure_project(
            &config.project.name,
            &canonical_root.to_string_lossy(),
            config.project.languages.first().map(|s| s.as_str()),
        )?;

        // Determine changed files
        let git_mgr = GitManager::new(&self.project_root);
        let target_tree = if let Some(hash) = snapshot_tree_hash {
            Some(hash.to_string())
        } else {
            let conn = db.inner();
            conn.query_row(
                "SELECT tree_hash FROM git_snapshots ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok()
        };

        let changed_files = if let Some(tree_hash) = target_tree {
            if let Ok(diffs) = git_mgr.diff_snapshot(&tree_hash) {
                diffs
                    .into_iter()
                    .filter(|d| {
                        !d.file_path.starts_with(".davr") && !d.file_path.starts_with(".git")
                    })
                    .map(|d| d.file_path)
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let analyzer = davr_impact::ImpactAnalyzer::new(max_depth);
        analyzer.analyze(&db, &project_id, &changed_files, Some(max_depth))
    }

    /// Runs repeat test execution to detect flaky tests
    pub async fn run_flaky_tests(
        &self,
        framework: Option<&str>,
        filter: Option<&str>,
        iterations: Option<usize>,
    ) -> Result<davr_flaky::FlakySuiteReport> {
        let config = Config::load_from_dir(&self.project_root)?;
        let runner = davr_flaky::FlakyTestRunner::new(config.flaky.iterations as usize);

        let timeout = if config.flaky.timeout_seconds > 0 {
            config.flaky.timeout_seconds
        } else {
            30
        };

        let report = runner
            .run_analysis(&self.project_root, framework, filter, iterations, timeout)
            .await?;

        let db_path = self.project_root.join(".davr").join("davr.db");
        if db_path.exists() {
            if let Ok(db) = Database::open(&db_path) {
                let canonical_root = self
                    .project_root
                    .canonicalize()
                    .unwrap_or_else(|_| self.project_root.clone());

                if let Ok(project_id) = db.ensure_project(
                    &config.project.name,
                    &canonical_root.to_string_lossy(),
                    config.project.languages.first().map(|s| s.as_str()),
                ) {
                    let _ = runner.record_flaky_run(&db, &project_id, &report);
                }
            }
        }

        Ok(report)
    }

    /// Runs database migrations idempotently
    pub fn db_migrate(&self) -> Result<()> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            let _ = self.init(false, None)?;
            return Ok(());
        }
        let mut db = Database::open(&db_path)?;
        db.apply_migrations()
    }

    /// Backs up the SQLite database to a destination file
    pub fn db_backup(&self, custom_path: Option<&Path>) -> Result<PathBuf> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Err(DavrError::Database(
                "Database does not exist. Run davr init first.".into(),
            ));
        }
        let db = Database::open(&db_path)?;
        let target_path = if let Some(p) = custom_path {
            p.to_path_buf()
        } else {
            let backup_dir = self.project_root.join(".davr").join("backups");
            let _ = fs::create_dir_all(&backup_dir);
            let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
            backup_dir.join(format!("davr_backup_{}.db", timestamp))
        };

        db.backup(&target_path)?;
        Ok(target_path)
    }

    /// Verifies database integrity and foreign key constraints
    pub fn db_verify(&self) -> Result<Vec<String>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Err(DavrError::Database("Database does not exist.".into()));
        }
        let db = Database::open(&db_path)?;
        db.verify_integrity()
    }

    /// Retrieves database table statistics and disk size
    pub fn db_stats(&self) -> Result<davr_storage::DatabaseStats> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Err(DavrError::Database("Database does not exist.".into()));
        }
        let db = Database::open(&db_path)?;
        db.get_stats(Some(&db_path))
    }

    /// Prunes snapshots, telemetry events, and verification runs per retention rules
    pub fn clean(
        &self,
        older_than_days: Option<u32>,
        include_all: bool,
    ) -> Result<davr_storage::CleanReport> {
        let config = Config::load_from_dir(&self.project_root)?;
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Ok(davr_storage::CleanReport {
                telemetry_events_pruned: 0,
                verification_runs_pruned: 0,
                sessions_pruned: 0,
                snapshots_pruned: 0,
            });
        }

        let db = Database::open(&db_path)?;
        let tel_days = older_than_days.unwrap_or(config.telemetry.retention_days);
        let ver_days = older_than_days.unwrap_or(config.telemetry.verification_retention_days);

        let mut report = db.prune_records(tel_days, ver_days, include_all)?;

        // Prune snapshots
        let git_mgr = GitManager::new(&self.project_root);
        if git_mgr.is_git_repo() {
            let canonical_root = self
                .project_root
                .canonicalize()
                .unwrap_or_else(|_| self.project_root.clone());
            if let Ok(project_id) = db.ensure_project(
                &config.project.name,
                &canonical_root.to_string_lossy(),
                config.project.languages.first().map(|s| s.as_str()),
            ) {
                let pruned_count = git_mgr
                    .prune_old_snapshots(&db, &project_id, config.git.max_snapshots_per_project)
                    .unwrap_or(0);
                report.snapshots_pruned = pruned_count;
            }
        }

        Ok(report)
    }

    /// Exports telemetry events for downstream analysis or logging
    pub fn export_telemetry(
        &self,
        session_id: Option<&str>,
        since_ms: Option<i64>,
    ) -> Result<Vec<serde_json::Value>> {
        let db_path = self.project_root.join(".davr").join("davr.db");
        if !db_path.exists() {
            return Ok(Vec::new());
        }
        let db = Database::open(&db_path)?;
        db.export_telemetry(session_id, since_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub files_indexed: usize,
    pub symbols_extracted: usize,
    pub dependency_edges: usize,
}
