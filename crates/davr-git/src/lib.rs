use chrono::Utc;
use davr_storage::Database;
use davr_types::{
    DavrError, FileState, ProjectId, Result, RollbackId, SessionId, SnapshotId, SnapshotReason,
};
use git2::{IndexAddOption, ObjectType, Oid, Repository, StatusOptions, Tree};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[cfg(windows)]
#[link(name = "advapi32")]
#[link(name = "crypt32")]
#[link(name = "rpcrt4")]
#[link(name = "user32")]
extern "C" {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub tree_hash: String,
    pub reason: SnapshotReason,
    pub dirty_before: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackScope {
    /// Default: A ∩ B (Intersection of working-tree diff and session-touched files)
    SessionIntersection,
    /// Forced: Full diff set A (CLI only, requires confirmation)
    Forced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackConflict {
    pub file_path: String,
    pub pre_snapshot_state: FileState,
    pub post_agent_state: FileState,
    pub current_state: FileState,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackOperation {
    RestoreFile {
        file_path: String,
        blob_oid: String,
        is_tracked_in_snapshot: bool,
    },
    DeleteFile {
        file_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackPlan {
    pub target_snapshot_tree: String,
    pub operations: Vec<RollbackOperation>,
    pub conflicts: Vec<RollbackConflict>,
    pub excluded_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReport {
    pub id: RollbackId,
    pub snapshot_id: SnapshotId,
    pub scope: RollbackScope,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub conflicted_files: Vec<RollbackConflict>,
    pub excluded_files: Vec<String>,
    pub status: String,
    pub dry_run: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiffSummary {
    pub file_path: String,
    pub change_type: String, // "modified" | "added" | "deleted"
}

// =====================================================================
// Pure Rollback Planner
// =====================================================================

pub struct RollbackPlanner;

impl RollbackPlanner {
    /// Pure planning function that deterministically computes rollback operations and conflicts
    pub fn plan(
        target_snapshot_tree: &str,
        diff_summaries: &[FileDiffSummary],
        snapshot_blobs: &HashMap<String, String>, // file_path -> blob_oid
        session_touched_files: &HashSet<String>,
        post_session_states: &HashMap<String, FileState>,
        current_states: &HashMap<String, FileState>,
        scope: RollbackScope,
        force: bool,
    ) -> RollbackPlan {
        let mut operations = Vec::new();
        let mut conflicts = Vec::new();
        let mut excluded_files = Vec::new();

        for item in diff_summaries {
            let path = &item.file_path;
            if path.starts_with(".davr") || path.starts_with(".git") {
                continue;
            }

            if scope == RollbackScope::SessionIntersection && !session_touched_files.contains(path)
            {
                excluded_files.push(path.clone());
                continue;
            }

            let pre_state = match snapshot_blobs.get(path) {
                Some(oid) => FileState::Present(oid.clone()),
                None => FileState::Missing,
            };

            let current_state = current_states
                .get(path)
                .cloned()
                .unwrap_or(FileState::Missing);

            if force {
                match &pre_state {
                    FileState::Present(blob_oid) => {
                        operations.push(RollbackOperation::RestoreFile {
                            file_path: path.clone(),
                            blob_oid: blob_oid.clone(),
                            is_tracked_in_snapshot: true,
                        });
                    }
                    FileState::Missing => {
                        operations.push(RollbackOperation::DeleteFile {
                            file_path: path.clone(),
                        });
                    }
                }
                continue;
            }

            // Conflict-aware planning using 3-way states (A -> B -> C)
            if let Some(post_state) = post_session_states.get(path) {
                if current_state == *post_state {
                    // Safe! Current state equals post-agent state (no subsequent edits)
                    match &pre_state {
                        FileState::Present(blob_oid) => {
                            operations.push(RollbackOperation::RestoreFile {
                                file_path: path.clone(),
                                blob_oid: blob_oid.clone(),
                                is_tracked_in_snapshot: true,
                            });
                        }
                        FileState::Missing => {
                            operations.push(RollbackOperation::DeleteFile {
                                file_path: path.clone(),
                            });
                        }
                    }
                } else {
                    // Conflict detected: State diverged after agent finished
                    let reason = match (&post_state, &current_state) {
                        (FileState::Present(_), FileState::Missing) => {
                            "File was deleted after agent session completed.".to_string()
                        }
                        (FileState::Missing, FileState::Present(_)) => {
                            "File was created or modified after agent session completed."
                                .to_string()
                        }
                        (FileState::Present(b_hash), FileState::Present(c_hash)) => {
                            format!(
                                "File content modified after agent session (post: {:.8}..., current: {:.8}...).",
                                b_hash, c_hash
                            )
                        }
                        (FileState::Missing, FileState::Missing) => {
                            "File state unexpectedly diverged.".to_string()
                        }
                    };

                    conflicts.push(RollbackConflict {
                        file_path: path.clone(),
                        pre_snapshot_state: pre_state,
                        post_agent_state: post_state.clone(),
                        current_state,
                        reason,
                    });
                }
            } else {
                // No post-session state recorded (e.g. manual rollback vs snapshot)
                match &pre_state {
                    FileState::Present(blob_oid) => {
                        operations.push(RollbackOperation::RestoreFile {
                            file_path: path.clone(),
                            blob_oid: blob_oid.clone(),
                            is_tracked_in_snapshot: true,
                        });
                    }
                    FileState::Missing => {
                        operations.push(RollbackOperation::DeleteFile {
                            file_path: path.clone(),
                        });
                    }
                }
            }
        }

        RollbackPlan {
            target_snapshot_tree: target_snapshot_tree.to_string(),
            operations,
            conflicts,
            excluded_files,
        }
    }
}

// =====================================================================
// Transactional Rollback Journal & Executor
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JournalStatus {
    Prepared,
    BackedUp,
    Applying,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RollbackJournalManifest {
    rollback_id: String,
    target_snapshot_tree: String,
    status: JournalStatus,
    operations: Vec<RollbackOperation>,
    backed_up_files: Vec<String>,
    created_at: i64,
}

pub struct RollbackExecutor<'a> {
    project_root: &'a Path,
    repo: &'a Repository,
}

impl<'a> RollbackExecutor<'a> {
    pub fn new(project_root: &'a Path, repo: &'a Repository) -> Self {
        Self { project_root, repo }
    }

    /// Executes a rollback plan transactionally with full backup journaling and crash recovery
    pub fn execute(&self, plan: &RollbackPlan, dry_run: bool) -> Result<RollbackReport> {
        let rollback_id = RollbackId::new();
        let snapshot_id = SnapshotId::from_string(&plan.target_snapshot_tree);

        if dry_run {
            let mut restored = Vec::new();
            let mut deleted = Vec::new();
            for op in &plan.operations {
                match op {
                    RollbackOperation::RestoreFile { file_path, .. } => {
                        restored.push(file_path.clone());
                    }
                    RollbackOperation::DeleteFile { file_path } => {
                        deleted.push(file_path.clone());
                    }
                }
            }
            return Ok(RollbackReport {
                id: rollback_id,
                snapshot_id,
                scope: RollbackScope::SessionIntersection,
                restored_files: restored,
                deleted_files: deleted,
                conflicted_files: plan.conflicts.clone(),
                excluded_files: plan.excluded_files.clone(),
                status: "succeeded".to_string(),
                dry_run: true,
                error_message: None,
            });
        }

        // 1. Prepare Transaction Directory
        let txn_dir = self
            .project_root
            .join(".davr")
            .join("rollback-txn")
            .join(rollback_id.as_str());
        let backups_dir = txn_dir.join("backups");
        fs::create_dir_all(&backups_dir).map_err(|e| {
            DavrError::Git(format!(
                "Failed to create rollback transaction journal: {}",
                e
            ))
        })?;

        let manifest_path = txn_dir.join("manifest.json");
        let mut manifest = RollbackJournalManifest {
            rollback_id: rollback_id.as_str().to_string(),
            target_snapshot_tree: plan.target_snapshot_tree.clone(),
            status: JournalStatus::Prepared,
            operations: plan.operations.clone(),
            backed_up_files: Vec::new(),
            created_at: Utc::now().timestamp_millis(),
        };
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .map_err(|e| DavrError::Git(e.to_string()))?;

        // 2. Validate path containment & Stage Backups
        for op in &plan.operations {
            let rel_path = match op {
                RollbackOperation::RestoreFile { file_path, .. } => file_path,
                RollbackOperation::DeleteFile { file_path } => file_path,
            };

            let validated_target = validate_path_containment(self.project_root, rel_path)?;
            if validated_target.exists() && !validated_target.is_dir() {
                let backup_file = backups_dir.join(rel_path);
                if let Some(parent) = backup_file.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::copy(&validated_target, &backup_file).map_err(|e| {
                    DavrError::Git(format!("Failed to backup file {}: {}", rel_path, e))
                })?;
                manifest.backed_up_files.push(rel_path.clone());
            }
        }

        manifest.status = JournalStatus::BackedUp;
        let _ = fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        );

        // 3. Apply Operations
        manifest.status = JournalStatus::Applying;
        let _ = fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        );

        let mut restored = Vec::new();
        let mut deleted = Vec::new();
        let mut apply_error = None;

        for op in &plan.operations {
            match op {
                RollbackOperation::RestoreFile {
                    file_path,
                    blob_oid,
                    ..
                } => {
                    let target = self.project_root.join(file_path);
                    if let Some(parent) = target.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    match Oid::from_str(blob_oid).and_then(|oid| self.repo.find_blob(oid)) {
                        Ok(blob) => {
                            if let Err(e) = fs::write(&target, blob.content()) {
                                apply_error = Some(format!(
                                    "Failed to write restored file {}: {}",
                                    file_path, e
                                ));
                                break;
                            }
                            restored.push(file_path.clone());
                        }
                        Err(e) => {
                            apply_error = Some(format!(
                                "Failed to find blob {} for {}: {}",
                                blob_oid, file_path, e
                            ));
                            break;
                        }
                    }
                }
                RollbackOperation::DeleteFile { file_path } => {
                    let target = self.project_root.join(file_path);
                    if target.exists() {
                        if let Err(e) = fs::remove_file(&target) {
                            apply_error =
                                Some(format!("Failed to remove added file {}: {}", file_path, e));
                            break;
                        }
                    }
                    deleted.push(file_path.clone());
                }
            }
        }

        // 4. Handle Failure / Recovery
        if let Some(err_msg) = apply_error {
            warn!(err = %err_msg, "Rollback failed during apply! Recovering from backups...");
            // Restore from backups
            for rel_path in &manifest.backed_up_files {
                let backup_file = backups_dir.join(rel_path);
                let target = self.project_root.join(rel_path);
                if backup_file.exists() {
                    let _ = fs::copy(&backup_file, &target);
                }
            }
            manifest.status = JournalStatus::Aborted;
            let _ = fs::write(
                &manifest_path,
                serde_json::to_string_pretty(&manifest).unwrap(),
            );

            return Ok(RollbackReport {
                id: rollback_id,
                snapshot_id,
                scope: RollbackScope::SessionIntersection,
                restored_files: Vec::new(),
                deleted_files: Vec::new(),
                conflicted_files: plan.conflicts.clone(),
                excluded_files: plan.excluded_files.clone(),
                status: "failed".to_string(),
                dry_run: false,
                error_message: Some(err_msg),
            });
        }

        // 5. Commit Transaction
        manifest.status = JournalStatus::Committed;
        let _ = fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        );
        let _ = fs::remove_dir_all(&txn_dir); // Clean up transaction journal after commit

        Ok(RollbackReport {
            id: rollback_id,
            snapshot_id,
            scope: RollbackScope::SessionIntersection,
            restored_files: restored,
            deleted_files: deleted,
            conflicted_files: plan.conflicts.clone(),
            excluded_files: plan.excluded_files.clone(),
            status: "succeeded".to_string(),
            dry_run: false,
            error_message: None,
        })
    }
}

// =====================================================================
// GitManager
// =====================================================================

pub struct GitManager {
    project_root: PathBuf,
}

impl GitManager {
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
        }
    }

    /// Checks if directory is inside a valid git repository
    pub fn is_git_repo(&self) -> bool {
        Repository::discover(&self.project_root).is_ok()
    }

    /// Checks if working tree is dirty (staged or unstaged changes)
    pub fn is_dirty(&self) -> Result<bool> {
        let repo = Repository::discover(&self.project_root)
            .map_err(|e| DavrError::Git(format!("Failed to open git repo: {}", e)))?;
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        let statuses = repo
            .statuses(Some(&mut opts))
            .map_err(|e| DavrError::Git(format!("Failed to get repo status: {}", e)))?;
        Ok(!statuses.is_empty())
    }

    /// Returns the current Git branch name, or None if detached HEAD or not a repo
    pub fn current_branch(&self) -> Option<String> {
        let repo = Repository::discover(&self.project_root).ok()?;
        let head = repo.head().ok()?;
        head.shorthand().map(|s| s.to_string())
    }

    /// Creates a content-addressed snapshot tree capturing staged, unstaged, and untracked files
    pub fn create_snapshot(
        &self,
        db: Option<&Database>,
        project_id: Option<&ProjectId>,
        session_id: Option<&SessionId>,
        reason: SnapshotReason,
    ) -> Result<Snapshot> {
        let repo = Repository::discover(&self.project_root)
            .map_err(|e| DavrError::Git(format!("Not a git repository: {}", e)))?;

        let dirty_before = self.is_dirty()?;
        let mut index = repo
            .index()
            .map_err(|e| DavrError::Git(format!("Failed to read git index: {}", e)))?;

        // Add all tracked, untracked, and modified files (respecting .gitignore)
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|e| DavrError::Git(format!("Failed to index working tree: {}", e)))?;

        let tree_oid = index
            .write_tree_to(&repo)
            .map_err(|e| DavrError::Git(format!("Failed to write tree object: {}", e)))?;

        let snapshot_id = SnapshotId::new();
        let now = Utc::now().timestamp_millis();
        let tree_hash = tree_oid.to_string();

        // Create ref to prevent garbage collection
        let refname = format!("refs/davr/snapshots/{}", snapshot_id.as_str());
        let _ = repo.reference(&refname, tree_oid, true, "DAVR safety snapshot");

        if let (Some(database), Some(proj_id)) = (db, project_id) {
            let session_str = session_id.map(|s| s.as_str());
            let conn = database.inner();
            conn.execute(
                "INSERT INTO git_snapshots (id, project_id, session_id, tree_hash, reason, dirty_before, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    snapshot_id.as_str(),
                    proj_id.as_str(),
                    session_str,
                    tree_hash,
                    reason.to_string(),
                    if dirty_before { 1 } else { 0 },
                    now
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

            // Record all individual file versions for the snapshot
            let tree = repo
                .find_tree(tree_oid)
                .map_err(|e| DavrError::Git(e.to_string()))?;
            let _ = record_tree_versions(conn, proj_id, &snapshot_id, &repo, &tree, Path::new(""));
        }

        info!(
            snapshot_id = %snapshot_id,
            tree_hash = %tree_hash,
            reason = %reason,
            dirty = dirty_before,
            "Captured Git ODB snapshot"
        );

        Ok(Snapshot {
            id: snapshot_id,
            tree_hash,
            reason,
            dirty_before,
            created_at: now,
        })
    }

    /// Prunes old snapshots to enforce max_snapshots_per_project limit
    pub fn prune_old_snapshots(
        &self,
        db: &Database,
        project_id: &ProjectId,
        max_snapshots: usize,
    ) -> Result<usize> {
        if max_snapshots == 0 {
            return Ok(0);
        }

        let repo = match Repository::discover(&self.project_root) {
            Ok(r) => r,
            Err(_) => return Ok(0),
        };

        let conn = db.inner();
        let mut stmt = conn
            .prepare(
                "SELECT id FROM git_snapshots
                 WHERE project_id = ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![project_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| DavrError::Database(e.to_string()))?;

        let mut all_ids = Vec::new();
        for id in rows.flatten() {
            all_ids.push(id);
        }

        if all_ids.len() <= max_snapshots {
            return Ok(0);
        }

        let to_prune = &all_ids[max_snapshots..];
        let mut pruned = 0;

        for snap_id in to_prune {
            // Delete Git ref
            let refname = format!("refs/davr/snapshots/{}", snap_id);
            if let Ok(mut r) = repo.find_reference(&refname) {
                let _ = r.delete();
            }
            // Delete from DB
            let _ = conn.execute("DELETE FROM git_snapshots WHERE id = ?1", [snap_id]);
            pruned += 1;
        }

        info!(
            pruned = pruned,
            "Pruned old Git snapshots according to retention policy"
        );
        Ok(pruned)
    }

    /// Computes file diffs between snapshot tree and current working directory
    pub fn diff_snapshot(&self, snapshot_tree_hash: &str) -> Result<Vec<FileDiffSummary>> {
        let repo = Repository::discover(&self.project_root)
            .map_err(|e| DavrError::Git(format!("Not a git repository: {}", e)))?;

        let oid = Oid::from_str(snapshot_tree_hash)
            .map_err(|e| DavrError::Git(format!("Invalid tree hash: {}", e)))?;
        let snap_tree = repo
            .find_tree(oid)
            .map_err(|e| DavrError::Git(format!("Snapshot tree not found: {}", e)))?;

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.include_untracked(true);
        diff_opts.recurse_untracked_dirs(true);

        let diff = repo
            .diff_tree_to_workdir(Some(&snap_tree), Some(&mut diff_opts))
            .map_err(|e| DavrError::Git(format!("Failed to compute diff: {}", e)))?;

        let mut summaries = Vec::new();
        diff.foreach(
            &mut |delta, _| {
                let change_type = match delta.status() {
                    git2::Delta::Added | git2::Delta::Untracked => "added",
                    git2::Delta::Deleted => "deleted",
                    git2::Delta::Modified => "modified",
                    git2::Delta::Renamed => "renamed",
                    _ => "modified",
                };
                if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
                    let path_str = path.to_string_lossy().to_string();
                    if !path_str.starts_with(".davr") && !path_str.starts_with(".git") {
                        summaries.push(FileDiffSummary {
                            file_path: path_str,
                            change_type: change_type.to_string(),
                        });
                    }
                }
                true
            },
            None,
            None,
            None,
        )
        .map_err(|e| DavrError::Git(format!("Failed to iterate diff: {}", e)))?;

        Ok(summaries)
    }

    /// Fetches map of relative path -> blob oid from a snapshot tree
    pub fn get_snapshot_blobs(&self, snapshot_tree_hash: &str) -> Result<HashMap<String, String>> {
        let repo = Repository::discover(&self.project_root)
            .map_err(|e| DavrError::Git(format!("Not a git repository: {}", e)))?;
        let oid = Oid::from_str(snapshot_tree_hash)
            .map_err(|e| DavrError::Git(format!("Invalid tree hash: {}", e)))?;
        let snap_tree = repo
            .find_tree(oid)
            .map_err(|e| DavrError::Git(format!("Snapshot tree not found: {}", e)))?;

        let mut map = HashMap::new();
        collect_tree_blobs(&repo, &snap_tree, Path::new(""), &mut map);
        Ok(map)
    }

    /// Rolls back working tree safely using RollbackPlanner and transactional RollbackExecutor
    pub fn rollback(
        &self,
        snapshot_tree_hash: &str,
        session_touched_files: &HashSet<String>,
        post_session_states: &HashMap<String, FileState>,
        scope: RollbackScope,
        dry_run: bool,
        force: bool,
    ) -> Result<RollbackReport> {
        let repo = Repository::discover(&self.project_root)
            .map_err(|e| DavrError::Git(format!("Not a git repository: {}", e)))?;

        let diff_summaries = self.diff_snapshot(snapshot_tree_hash)?;
        let snapshot_blobs = self.get_snapshot_blobs(snapshot_tree_hash)?;

        // Compute current states on disk
        let mut current_states = HashMap::new();
        for item in &diff_summaries {
            let p = self.project_root.join(&item.file_path);
            if p.exists() && !p.is_dir() {
                if let Ok(bytes) = fs::read(&p) {
                    current_states.insert(
                        item.file_path.clone(),
                        FileState::Present(blake3::hash(&bytes).to_hex().to_string()),
                    );
                }
            } else {
                current_states.insert(item.file_path.clone(), FileState::Missing);
            }
        }

        let plan = RollbackPlanner::plan(
            snapshot_tree_hash,
            &diff_summaries,
            &snapshot_blobs,
            session_touched_files,
            post_session_states,
            &current_states,
            scope,
            force,
        );

        let executor = RollbackExecutor::new(&self.project_root, &repo);
        executor.execute(&plan, dry_run)
    }
}

// =====================================================================
// Internal Helpers
// =====================================================================

fn collect_tree_blobs(
    repo: &Repository,
    tree: &Tree,
    prefix: &Path,
    map: &mut HashMap<String, String>,
) {
    for entry in tree.iter() {
        let name = entry.name().unwrap_or("");
        let path = prefix.join(name);
        match entry.kind() {
            Some(ObjectType::Blob) => {
                map.insert(path.to_string_lossy().to_string(), entry.id().to_string());
            }
            Some(ObjectType::Tree) => {
                if let Ok(obj) = entry.to_object(repo) {
                    if let Some(sub) = obj.as_tree() {
                        collect_tree_blobs(repo, sub, &path, map);
                    }
                }
            }
            _ => {}
        }
    }
}

fn record_tree_versions(
    conn: &rusqlite::Connection,
    project_id: &ProjectId,
    snapshot_id: &SnapshotId,
    repo: &Repository,
    tree: &Tree,
    prefix: &Path,
) -> Result<()> {
    let now = Utc::now().timestamp_millis();
    for entry in tree.iter() {
        let name = entry.name().unwrap_or("");
        let path = prefix.join(name);
        match entry.kind() {
            Some(ObjectType::Blob) => {
                let blob = repo
                    .find_blob(entry.id())
                    .map_err(|e| DavrError::Git(e.to_string()))?;
                let blob_hash = entry.id().to_string();
                let size_bytes = blob.size() as i64;
                conn.execute(
                    "INSERT INTO file_versions (project_id, snapshot_id, file_path, blob_hash, size_bytes, recorded_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        project_id.as_str(),
                        snapshot_id.as_str(),
                        path.to_string_lossy(),
                        blob_hash,
                        size_bytes,
                        now
                    ],
                )
                .map_err(|e| DavrError::Database(e.to_string()))?;
            }
            Some(ObjectType::Tree) => {
                let sub_tree = repo
                    .find_tree(entry.id())
                    .map_err(|e| DavrError::Git(e.to_string()))?;
                record_tree_versions(conn, project_id, snapshot_id, repo, &sub_tree, &path)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_path_containment(project_root: &Path, rel_path: &str) -> Result<PathBuf> {
    let path = Path::new(rel_path);
    for comp in path.components() {
        if let std::path::Component::ParentDir = comp {
            return Err(DavrError::Security(format!(
                "Path traversal detected: {}",
                rel_path
            )));
        }
    }

    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let target = canonical_root.join(rel_path);

    if target.exists() {
        let canonical_target = target.canonicalize().map_err(|e| {
            DavrError::Security(format!("Failed to canonicalize path {}: {}", rel_path, e))
        })?;
        if !canonical_target.starts_with(&canonical_root) {
            return Err(DavrError::Security(format!(
                "Symlink points outside repository: {}",
                rel_path
            )));
        }
    }

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_rollback_planner_conflicts() {
        let diffs = vec![FileDiffSummary {
            file_path: "auth.rs".into(),
            change_type: "modified".into(),
        }];

        let mut blobs = HashMap::new();
        blobs.insert("auth.rs".into(), "blob_a".into());

        let mut touched = HashSet::new();
        touched.insert("auth.rs".into());

        let mut post_states = HashMap::new();
        post_states.insert("auth.rs".into(), FileState::Present("hash_b".into()));

        // Case 1: Current == B (Safe)
        let mut curr_safe = HashMap::new();
        curr_safe.insert("auth.rs".into(), FileState::Present("hash_b".into()));

        let plan_safe = RollbackPlanner::plan(
            "tree_1",
            &diffs,
            &blobs,
            &touched,
            &post_states,
            &curr_safe,
            RollbackScope::SessionIntersection,
            false,
        );
        assert_eq!(plan_safe.operations.len(), 1);
        assert_eq!(plan_safe.conflicts.len(), 0);

        // Case 2: Current == C != B (Conflict)
        let mut curr_conflict = HashMap::new();
        curr_conflict.insert("auth.rs".into(), FileState::Present("hash_c".into()));

        let plan_conflict = RollbackPlanner::plan(
            "tree_1",
            &diffs,
            &blobs,
            &touched,
            &post_states,
            &curr_conflict,
            RollbackScope::SessionIntersection,
            false,
        );
        assert_eq!(plan_conflict.operations.len(), 0);
        assert_eq!(plan_conflict.conflicts.len(), 1);
        assert_eq!(plan_conflict.conflicts[0].file_path, "auth.rs");

        // Case 3: Forced on Conflict
        let plan_forced = RollbackPlanner::plan(
            "tree_1",
            &diffs,
            &blobs,
            &touched,
            &post_states,
            &curr_conflict,
            RollbackScope::SessionIntersection,
            true,
        );
        assert_eq!(plan_forced.operations.len(), 1);
        assert_eq!(plan_forced.conflicts.len(), 0);
    }
}
