use davr_config::Config;
use davr_core::CoreEngine;
use davr_git::RollbackScope;
use davr_storage::Database;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn setup_test_git_repo(root: &std::path::Path) {
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root)
        .status()
        .expect("git init failed");
    assert!(status.success());

    let _ = Command::new("git")
        .args(["config", "user.name", "DAVR Matrix Test"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@davr.dev"])
        .current_dir(root)
        .status();
}

#[tokio::test]
async fn test_matrix_same_file_concurrent_edit_conflict_and_force() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    setup_test_git_repo(root);

    let auth_file = root.join("auth.rs");
    fs::write(&auth_file, b"version 1 (pre-session)\n").unwrap();

    let _ = Command::new("git").args(["add", "."]).current_dir(root).status();
    let _ = Command::new("git").args(["commit", "-m", "init"]).current_dir(root).status();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();

    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(root.join(".davr/config.toml"), config.to_toml_string().unwrap()).unwrap();

    // 1. Agent modifies auth.rs -> Version 2
    let summary = engine
        .run_agent_session(
            Some("generic"),
            "bash",
            &["-c".into(), "echo 'version 2 (agent edit)' > auth.rs".into()],
            false,
        )
        .await
        .expect("Agent session failed");

    // 2. Developer modifies the SAME file -> Version 3
    fs::write(&auth_file, b"version 3 (developer edit after agent)\n").unwrap();

    // 3. Rollback without force -> MUST detect conflict and preserve developer's Version 3!
    let report = engine
        .rollback(
            None,
            Some(&summary.session_id),
            RollbackScope::SessionIntersection,
            false,
            false, // force = false
        )
        .expect("Rollback failed");

    assert_eq!(report.conflicted_files.len(), 1, "Must detect conflict on auth.rs");
    assert_eq!(report.conflicted_files[0].file_path, "auth.rs");
    assert_eq!(
        fs::read_to_string(&auth_file).unwrap(),
        "version 3 (developer edit after agent)\n",
        "Developer Version 3 must NOT be overwritten on normal rollback!"
    );

    // 4. Rollback WITH force -> Overwrites Version 3 with Version 1 snapshot
    let forced_report = engine
        .rollback(
            None,
            Some(&summary.session_id),
            RollbackScope::SessionIntersection,
            false,
            true, // force = true
        )
        .expect("Forced rollback failed");

    assert_eq!(forced_report.status, "succeeded");
    assert!(forced_report.restored_files.contains(&"auth.rs".to_string()));
    assert_eq!(
        fs::read_to_string(&auth_file).unwrap(),
        "version 1 (pre-session)\n",
        "Forced rollback must restore original snapshot version!"
    );
}

#[tokio::test]
async fn test_matrix_agent_created_file_modified_by_developer_is_conflict() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    setup_test_git_repo(root);

    let seed = root.join("seed.txt");
    fs::write(&seed, b"seed\n").unwrap();
    let _ = Command::new("git").args(["add", "."]).current_dir(root).status();
    let _ = Command::new("git").args(["commit", "-m", "init"]).current_dir(root).status();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();
    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(root.join(".davr/config.toml"), config.to_toml_string().unwrap()).unwrap();

    // 1. Agent creates new_feature.rs
    let summary = engine
        .run_agent_session(
            Some("generic"),
            "bash",
            &["-c".into(), "echo 'agent feature' > new_feature.rs".into()],
            false,
        )
        .await
        .unwrap();

    // 2. Developer modifies new_feature.rs
    let feature_file = root.join("new_feature.rs");
    fs::write(&feature_file, b"developer expanded feature\n").unwrap();

    // 3. Rollback without force -> MUST detect conflict and NOT delete new_feature.rs
    let report = engine
        .rollback(
            None,
            Some(&summary.session_id),
            RollbackScope::SessionIntersection,
            false,
            false,
        )
        .unwrap();

    assert_eq!(report.conflicted_files.len(), 1);
    assert!(feature_file.exists(), "Developer-modified feature file must NOT be deleted!");
    assert_eq!(
        fs::read_to_string(&feature_file).unwrap(),
        "developer expanded feature\n"
    );
}

#[tokio::test]
async fn test_matrix_secret_redaction_in_persistence() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    setup_test_git_repo(root);

    let seed = root.join("seed.txt");
    fs::write(&seed, b"seed\n").unwrap();
    let _ = Command::new("git").args(["add", "."]).current_dir(root).status();
    let _ = Command::new("git").args(["commit", "-m", "init"]).current_dir(root).status();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();
    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(root.join(".davr/config.toml"), config.to_toml_string().unwrap()).unwrap();

    // Run agent with OpenAI secret in arguments
    let summary = engine
        .run_agent_session(
            Some("generic"),
            "echo",
            &["running with --api-key sk-abcdef1234567890abcdef1234567890".into()],
            false,
        )
        .await
        .unwrap();

    // Verify in SQLite database that raw secret was REDACTED before persistence
    let db = Database::open(&root.join(".davr/davr.db")).unwrap();
    let conn = db.inner();
    let cmd: String = conn
        .query_row(
            "SELECT command_line FROM agent_sessions WHERE id = ?1",
            [&summary.session_id],
            |row| row.get(0),
        )
        .unwrap();

    assert!(!cmd.contains("sk-abcdef1234567890"), "Raw secret must NOT exist in DB!");
    assert!(cmd.contains("[REDACTED]"), "Command line must contain [REDACTED]");
}

#[tokio::test]
async fn test_matrix_top_level_security_policy_blocking() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    setup_test_git_repo(root);

    let seed = root.join("seed.txt");
    fs::write(&seed, b"seed\n").unwrap();
    let _ = Command::new("git").args(["add", "."]).current_dir(root).status();
    let _ = Command::new("git").args(["commit", "-m", "init"]).current_dir(root).status();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();
    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    // Configure blocked command pattern
    config.security.blocked_commands = vec![r"rm\s+-rf\s+/.*".into()];
    fs::write(root.join(".davr/config.toml"), config.to_toml_string().unwrap()).unwrap();

    // Attempt to run blocked command
    let res = engine
        .run_agent_session(Some("generic"), "rm", &["-rf".into(), "/tmp/test".into()], false)
        .await;

    assert!(res.is_err(), "Blocked command must return error before spawning");
    if let Err(e) = res {
        assert!(e.to_string().contains("blocked by security policy"));
    }
}

#[tokio::test]
async fn test_matrix_rollback_audit_history_recorded() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    setup_test_git_repo(root);

    let file = root.join("doc.txt");
    fs::write(&file, b"initial\n").unwrap();
    let _ = Command::new("git").args(["add", "."]).current_dir(root).status();
    let _ = Command::new("git").args(["commit", "-m", "init"]).current_dir(root).status();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();
    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(root.join(".davr/config.toml"), config.to_toml_string().unwrap()).unwrap();

    let summary = engine
        .run_agent_session(
            Some("generic"),
            "bash",
            &["-c".into(), "echo 'agent' > doc.txt".into()],
            false,
        )
        .await
        .unwrap();

    let _ = engine
        .rollback(
            None,
            Some(&summary.session_id),
            RollbackScope::SessionIntersection,
            false,
            false,
        )
        .unwrap();

    let history = engine.list_rollbacks(10).unwrap();
    assert_eq!(history.len(), 1, "Must record exactly 1 rollback operation in audit log");
    assert_eq!(history[0].status, "succeeded");
    assert_eq!(history[0].files_restored_count, 1);
}
