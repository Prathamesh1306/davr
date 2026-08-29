use davr_core::CoreEngine;
use davr_env::EnvironmentValidator;
use davr_git::GitManager;
use davr_types::CheckStatus;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
}

#[test]
fn test_broken_env_fixture_reports_expected_failures() {
    let fixture_path = fixtures_dir().join("broken-env");
    assert!(fixture_path.exists(), "broken-env fixture must exist");

    let engine = CoreEngine::new(&fixture_path);
    let results = engine.doctor(None).expect("doctor check failed");

    // Must fail on nonexistent tool
    let tool_check = results
        .iter()
        .find(|r| r.name.contains("nonexistent_compiler_binary"))
        .expect("Missing tool check for nonexistent_compiler_binary");
    assert_eq!(tool_check.status, CheckStatus::Fail);

    // Must fail on missing env var
    let env_check = results
        .iter()
        .find(|r| r.name.contains("NONEXISTENT_REQUIRED_SECRET_VAR"))
        .expect("Missing env var check for NONEXISTENT_REQUIRED_SECRET_VAR");
    assert_eq!(env_check.status, CheckStatus::Fail);

    // Overall has failures
    assert!(results.iter().any(|r| r.status == CheckStatus::Fail));
}

#[test]
fn test_language_detection_on_fixtures() {
    let validator = EnvironmentValidator::new();

    let rust_fixture = fixtures_dir().join("rust-project");
    let rust_langs = validator.detect_languages(&rust_fixture);
    assert_eq!(rust_langs, vec!["rust"]);

    let node_fixture = fixtures_dir().join("node-project");
    let node_langs = validator.detect_languages(&node_fixture);
    assert_eq!(node_langs, vec!["typescript"]);

    let py_fixture = fixtures_dir().join("python-project");
    let py_langs = validator.detect_languages(&py_fixture);
    assert_eq!(py_langs, vec!["python"]);
}

#[test]
fn test_ast_indexing_on_fixture_codebases() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Copy rust-project into temp dir
    let rust_fixture = fixtures_dir().join("rust-project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::copy(rust_fixture.join("Cargo.toml"), root.join("Cargo.toml")).unwrap();
    fs::copy(rust_fixture.join("src/lib.rs"), root.join("src/lib.rs")).unwrap();

    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();

    let summary = engine.analyze_project().expect("analyze_project failed");
    assert!(summary.files_indexed >= 1);
    assert!(summary.symbols_extracted >= 2); // 'add' and 'multiply'
}

#[test]
fn test_dirty_git_state_snapshot_captures_all_layers() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Init git repo
    let _ = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "DAVR Test"])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@davr.dev"])
        .current_dir(root)
        .status();

    // 1. Initial committed file
    let committed = root.join("committed.txt");
    fs::write(&committed, b"committed content v1\n").unwrap();
    let gitignore = root.join(".gitignore");
    fs::write(&gitignore, b"*.log\n.davr/\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .status();

    // 2. Unstaged modification to committed.txt
    fs::write(&committed, b"committed content v2 (unstaged)\n").unwrap();

    // 3. Staged new file
    let staged = root.join("staged.txt");
    fs::write(&staged, b"staged file content\n").unwrap();
    let _ = Command::new("git")
        .args(["add", "staged.txt"])
        .current_dir(root)
        .status();

    // 4. Untracked file (not ignored)
    let untracked = root.join("untracked.txt");
    fs::write(&untracked, b"untracked file content\n").unwrap();

    // 5. Ignored file
    let ignored = root.join("debug.log");
    fs::write(&ignored, b"ignored log\n").unwrap();

    // Verify git manager sees dirty state
    let git_mgr = GitManager::new(root);
    assert!(git_mgr.is_dirty().expect("is_dirty failed"));

    // Initialize DAVR and create snapshot
    let engine = CoreEngine::new(root);
    let _ = engine.init(false, Some(vec!["rust".into()])).unwrap();

    let snap = engine
        .create_snapshot(Some("dirty git test"))
        .expect("create_snapshot failed");
    assert!(snap.dirty_before);

    let detail = engine
        .get_snapshot_detail(&snap.id)
        .expect("get_snapshot_detail failed");

    // All tracked (unstaged & staged) and untracked files must be captured
    assert!(
        detail.files.iter().any(|f| f.ends_with("committed.txt")),
        "Committed file must be in snapshot"
    );
    assert!(
        detail.files.iter().any(|f| f.ends_with("staged.txt")),
        "Staged file must be in snapshot"
    );
    assert!(
        detail.files.iter().any(|f| f.ends_with("untracked.txt")),
        "Untracked file must be in snapshot"
    );
    // Ignored file must NOT be captured
    assert!(
        !detail.files.iter().any(|f| f.ends_with("debug.log")),
        "Ignored file must NOT be in snapshot"
    );
}
