use davr_config::Config;
use davr_core::CoreEngine;
use std::fs;
use tempfile::TempDir;

#[tokio::test]
async fn test_live_token_and_context_scraping_e2e() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    // 1. Initialize Git and DAVR project
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "DAVR Tester"])
        .current_dir(root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "tester@davr.dev"])
        .current_dir(root)
        .output();

    fs::write(root.join("README.md"), "# Test Project\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output();

    let engine = CoreEngine::new(root);
    engine.init(false, None).unwrap();

    let mut config = Config::load_from_dir(root).unwrap();
    config.environment.required_env_vars.clear();
    fs::write(
        root.join(".davr/config.toml"),
        config.to_toml_string().unwrap(),
    )
    .unwrap();

    // 2. Run agent session simulating Claude/Aider live stdout with tokens and context warning
    #[cfg(windows)]
    let (cmd, args) = (
        "cmd.exe",
        vec![
            "/C".into(),
            "echo Tokens: 1,420 input, 380 output. Cost: $0.024. Context fill ratio: 0.85".into(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".into(),
            "echo 'Tokens: 1,420 input, 380 output. Cost: $0.024. Context fill ratio: 0.85'".into(),
        ],
    );

    let summary = engine
        .run_agent_session(Some("generic"), cmd, &args, false)
        .await
        .expect("Agent session execution failed");

    // 3. Verify SessionSummary has parsed token usage & context metrics
    assert_eq!(summary.exit_code, 0);

    let tokens = summary
        .token_usage
        .expect("Expected token_usage to be scraped");
    assert_eq!(tokens.input_tokens, Some(1420));
    assert_eq!(tokens.output_tokens, Some(380));
    assert_eq!(tokens.total_tokens, Some(1800));
    assert_eq!(tokens.cost_usd, Some(0.024));

    let ctx = summary
        .context_metrics
        .expect("Expected context_metrics to be scraped");
    assert!((ctx.context_fill_ratio.unwrap() - 0.85).abs() < 1e-4);
    assert!(ctx.warning_triggered);

    // 4. Verify get_session_detail returns identical token metrics
    let detail = engine
        .get_session_detail(&summary.session_id)
        .expect("Failed to get session detail");
    assert!(detail.token_usage.is_some());
    let detail_tokens = detail.token_usage.unwrap();
    assert_eq!(detail_tokens.input_tokens, Some(1420));
    assert_eq!(detail_tokens.total_tokens, Some(1800));
    assert_eq!(detail_tokens.cost_usd, Some(0.024));

    assert!(detail.context_metrics.is_some());
    let detail_ctx = detail.context_metrics.unwrap();
    assert!(detail_ctx.warning_triggered);

    // 5. Verify telemetry trace contains TOKEN_USAGE and CONTEXT_WARNING
    let traces = engine
        .get_trace(Some(&summary.session_id), None)
        .expect("Failed to get telemetry trace");
    let kinds: Vec<String> = traces.into_iter().map(|t| t.kind).collect();
    assert!(
        kinds.contains(&"TOKEN_USAGE".to_string()),
        "Expected TOKEN_USAGE in telemetry events: {:?}",
        kinds
    );
    assert!(
        kinds.contains(&"CONTEXT_WARNING".to_string()),
        "Expected CONTEXT_WARNING in telemetry events: {:?}",
        kinds
    );
}
