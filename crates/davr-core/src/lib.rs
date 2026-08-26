use chrono::Utc;
use davr_agent::ProcessSupervisor;
use davr_config::Config;
use davr_env::{CheckResult, EnvironmentValidator};
use davr_fs::FilesystemMonitor;
use davr_git::{FileDiffSummary, GitManager, RollbackReport, RollbackScope};
use davr_storage::Database;
use davr_telemetry::TelemetryEmitter;
use davr_types::{
    CheckCategory, CheckStatus, DavrError, ProjectId, Result, SessionId,
    SessionStatus, Severity, SnapshotReason,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

pub use davr_test::{TestCaseResult, TestCaseStatus, TestSuiteResult};
pub use davr_flaky::{FlakyCaseReport, FlakyClassification, FlakySuiteReport};

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
        let detected_langs = languages.unwrap_or_else(|| env_validator.detect_languages(&self.project_root));

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
                config.git.max_snapshots_per_project as usize,
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
                        if !d.file_path.starts_with(".davr") && touched_set.insert(d.file_path.clone()) {
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
            if exit_code == 0 { Severity::Info } else { Severity::Warn },
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
        for r in rows {
            if let Ok(item) = r {
                list.push(item);
            }
        }
        Ok(list)
    }

    /// Fetches telemetry trace events
    pub fn get_trace(&self, session_id: Option<&str>, kind: Option<&str>) -> Result<Vec<TraceItem>> {
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

        let mut stmt = conn.prepare(query).map_err(|e| DavrError::Database(e.to_string()))?;

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
        while let Some(row) = rows.next().map_err(|e| DavrError::Database(e.to_string()))? {
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
        for r in rows {
            if let Ok(item) = r {
                list.push(item);
            }
        }
        Ok(list)
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
            for r in rows {
                if let Ok(f) = r {
                    touched.insert(f);
                }
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
                resolved_session_id.as_ref().map(|s| SessionId::from_string(s.clone())).as_ref(),
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

        let results = runner
            .run(&self.project_root, framework_override, filter, timeout)
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
                    let _ = runner.record_test_run(&db, &project_id, None, &results);
                }
            }
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
                    .filter(|d| !d.file_path.starts_with(".davr") && !d.file_path.starts_with(".git"))
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub files_indexed: usize,
    pub symbols_extracted: usize,
    pub dependency_edges: usize,
}
