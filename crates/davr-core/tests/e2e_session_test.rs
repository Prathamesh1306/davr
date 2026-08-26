use davr_config::Config;
use davr_core::CoreEngine;
use davr_git::RollbackScope;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[tokio::test]
async fn test_end_to_end_session_and_rollback_intersection() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Initialize git repo
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root)
        .status()
        .expect("git init failed");
    assert!(status.success());

    let _ = Command::new("git")
        .args(["config", "user.name", "DAVR Test"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@davr.dev"])
        .current_dir(root)
        .status();

    let initial_file = root.join("main.txt");
    fs::write(&initial_file, b"initial main content\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "main.txt"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .status();

    // 2. Initialize DAVR with empty required_env_vars for testing
    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();

    let config_path = root.join(".davr/config.toml");
    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(&config_path, config.to_toml_string().unwrap()).unwrap();

    // 3. Run supervised session that modifies main.txt and creates agent_file.txt
    let summary = engine
        .run_agent_session(
            Some("generic"),
            "bash",
            &[
                "-c".into(),
                "echo 'agent edit' >> main.txt && echo 'agent created' > agent_file.txt".into(),
            ],
            false,
        )
        .await
        .expect("Agent session failed");

    assert_eq!(summary.exit_code, 0);
    assert_eq!(summary.status, "completed");
    assert!(summary.pre_snapshot_id.is_some());

    // 4. Simulate unrelated developer edit outside the agent session
    let dev_file = root.join("dev_notes.txt");
    fs::write(&dev_file, b"developer independent work\n").unwrap();

    // 5. Rollback with SessionIntersection (A ∩ B)
    let report = engine
        .rollback(
            None,
            Some(&summary.session_id),
            RollbackScope::SessionIntersection,
            false,
            false,
        )
        .expect("Rollback failed");

    assert_eq!(report.status, "succeeded");
    assert!(report.restored_files.contains(&"main.txt".to_string()));
    assert!(report.deleted_files.contains(&"agent_file.txt".to_string()));
    assert!(report.excluded_files.contains(&"dev_notes.txt".to_string()));

    // 6. Verify filesystem state
    let main_content = fs::read_to_string(&initial_file).unwrap();
    assert_eq!(main_content, "initial main content\n");
    assert!(
        !root.join("agent_file.txt").exists(),
        "Agent file must be deleted on rollback"
    );
    assert!(
        root.join("dev_notes.txt").exists(),
        "Developer file must NOT be touched"
    );
    assert_eq!(
        fs::read_to_string(&dev_file).unwrap(),
        "developer independent work\n"
    );
}
