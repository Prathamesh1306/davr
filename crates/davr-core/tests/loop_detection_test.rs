use davr_core::loop_detection::{LoopDetector, LoopHeuristic};
use davr_core::CoreEngine;
use davr_storage::Database;
use davr_types::SessionId;
use tempfile::TempDir;

#[test]
fn test_loop_detector_heuristics_unit() {
    let config = davr_config::LoopDetectionConfig::default();
    let mut detector = LoopDetector::new(config);

    // 1. Repeated identical commands (3 times in window)
    assert!(detector.record_command("echo test", 0).is_none());
    assert!(detector.record_command("ls -la", 0).is_none());
    assert!(detector.record_command("echo test", 0).is_none());
    let notice1 = detector.record_command("echo test", 0);
    assert!(notice1.is_some());
    let n1 = notice1.unwrap();
    assert_eq!(n1.heuristic, LoopHeuristic::RepeatedIdenticalCommands);
    assert_eq!(n1.count, 3);

    // 2. Repeated failures
    let mut fail_detector = LoopDetector::new(davr_config::LoopDetectionConfig::default());
    assert!(fail_detector.record_command("cargo test", 1).is_none());
    assert!(fail_detector.record_command("cargo test", 1).is_none());
    let notice2 = fail_detector.record_command("cargo test", 1);
    assert!(notice2.is_some());
    let n2 = notice2.unwrap();
    assert_eq!(n2.heuristic, LoopHeuristic::RepeatedFailures);
    assert_eq!(n2.count, 3);

    // 3. File cycle-back (reverting to previous hash)
    let mut cycle_detector = LoopDetector::new(davr_config::LoopDetectionConfig::default());
    assert!(cycle_detector
        .record_file_mutation("main.rs", "hash_init")
        .is_none());
    assert!(cycle_detector
        .record_file_mutation("main.rs", "hash_edit1")
        .is_none());
    let notice3 = cycle_detector.record_file_mutation("main.rs", "hash_init");
    assert!(notice3.is_some());
    let n3 = notice3.unwrap();
    assert_eq!(n3.heuristic, LoopHeuristic::FileCycleBack);

    // 4. Unchanged state across iterations
    let mut state_detector = LoopDetector::new(davr_config::LoopDetectionConfig::default());
    assert!(state_detector.record_iteration_state(0, 0).is_none());
    assert!(state_detector.record_iteration_state(0, 0).is_none());
    let notice4 = state_detector.record_iteration_state(0, 0);
    assert!(notice4.is_some());
    let n4 = notice4.unwrap();
    assert_eq!(n4.heuristic, LoopHeuristic::UnchangedState);

    // 5. Excessive retries without intervening changes
    let mut retry_detector = LoopDetector::new(davr_config::LoopDetectionConfig::default());
    retry_detector.record_file_mutation("foo.rs", "h1");
    assert!(retry_detector.record_test_execution().is_none());
    assert!(retry_detector.record_test_execution().is_none());
    let notice5 = retry_detector.record_test_execution();
    assert!(notice5.is_some());
    let n5 = notice5.unwrap();
    assert_eq!(n5.heuristic, LoopHeuristic::ExcessiveRetries);
}

#[tokio::test]
async fn test_e2e_session_loop_detection_integration() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // 1. Initialize DAVR engine in temp dir
    let engine = CoreEngine::new(root);
    engine.init(true, Some(vec!["rust".into()])).unwrap();

    let db_path = root.join(".davr").join("davr.db");
    let db = Database::open(&db_path).unwrap();
    let session_id = SessionId::new();

    // Create session record
    let project_id = db
        .ensure_project("test-loop", &root.to_string_lossy(), Some("rust"))
        .unwrap();
    db.create_session(&session_id, &project_id, "claude", "agent")
        .unwrap();

    // 2. Run repeated identical commands via exec
    for _ in 0..3 {
        let code = engine
            .exec(
                "echo",
                &["repeating-action".into()],
                Some(session_id.as_str()),
            )
            .await
            .unwrap();
        assert_eq!(code, 0);
    }

    // 3. Inspect session history with LoopDetector
    let mut detector = LoopDetector::new(davr_config::LoopDetectionConfig::default());
    let notices = detector.inspect_session(&db, &session_id).unwrap();
    assert!(
        notices
            .iter()
            .any(|n| n.heuristic == LoopHeuristic::RepeatedIdenticalCommands),
        "Expected RepeatedIdenticalCommands notice in session history"
    );

    // 4. Verify telemetry recorded LOOP_DETECTED event
    let events = engine
        .get_trace(Some(session_id.as_str()), Some("LOOP_DETECTED"))
        .unwrap();
    assert!(
        !events.is_empty(),
        "Expected LOOP_DETECTED event in SQLite telemetry"
    );
}

#[tokio::test]
async fn test_test_subflags_selected_and_full() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let engine = CoreEngine::new(root);
    engine.init(true, Some(vec!["rust".into()])).unwrap();

    // Disable fallback so selected_only returns Ok(empty) when 0 tests are impacted
    let config_path = root.join(".davr").join("config.toml");
    let mut config = davr_config::Config::load_from_dir(root).unwrap();
    config.test.fallback_to_full_suite = false;
    std::fs::write(&config_path, config.to_toml_string().unwrap()).unwrap();

    // Test with selected_only: true (no tests should run if no changes)
    let results = engine.run_tests(None, None, true, false).await.unwrap();
    assert!(
        results.is_empty(),
        "Expected 0 tests to run when selected_only without changes and fallback disabled"
    );
}
