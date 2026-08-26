use clap::{Args, Parser, Subcommand};
use colored::*;
use davr_config::{find_project_root, Config};
use davr_core::CoreEngine;
use davr_git::RollbackScope;
use davr_types::{CheckStatus, DavrError};
use std::path::PathBuf;
use std::process;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "davr",
    about = "DAVR — Deterministic Agent Verification Runtime\nAI generates. DAVR verifies.",
    version = "0.1.0"
)]
struct Cli {
    #[arg(long, global = true, help = "Override the project root directory")]
    project: Option<PathBuf>,

    #[arg(long, global = true, help = "Use an explicit config file")]
    config: Option<PathBuf>,

    #[arg(long, global = true, help = "Emit machine-readable JSON output")]
    json: bool,

    #[arg(long, global = true, help = "Disable ANSI color in human output")]
    no_color: bool,

    #[arg(short, long, global = true, help = "Suppress non-essential output")]
    quiet: bool,

    #[arg(short, long, global = true, help = "Increase diagnostic logging")]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Initialize .davr/ in the project root")]
    Init(InitArgs),

    #[command(about = "Run environment validation checks (standalone)")]
    Doctor(DoctorArgs),

    #[command(about = "Wrap and supervise an AI agent execution session")]
    Run(RunArgs),

    #[command(about = "Session management")]
    Session(SessionArgs),

    #[command(about = "Inspect telemetry event traces")]
    Trace(TraceArgs),

    #[command(about = "Manage Git snapshots")]
    Snapshot(SnapshotArgs),

    #[command(about = "Rollback working tree to a prior snapshot safely (A ∩ B)")]
    Rollback(RollbackArgs),

    #[command(about = "Compare differences between a snapshot and working tree")]
    Diff(DiffArgs),

    #[command(about = "Run test suites across detected frameworks")]
    Test(TestArgs),

    #[command(about = "Index and parse source AST symbols and dependency graph")]
    Analyze,

    #[command(about = "Perform transitive change impact analysis")]
    Impact(ImpactArgs),

    #[command(about = "Run flakiness stress analysis over repeat iterations")]
    Flaky(FlakyArgs),

    #[command(about = "Start Model Context Protocol (MCP) stdio JSON-RPC server")]
    Mcp,

    #[command(about = "Configuration management")]
    Config(ConfigArgs),

    #[command(about = "Print version and embedded build metadata")]
    Version,
}

#[derive(Args, Debug)]
struct FlakyArgs {
    #[arg(long, help = "Override framework (cargo_test, pytest, jest, go_test)")]
    framework: Option<String>,

    #[arg(short, long, help = "Filter pattern for test names")]
    filter: Option<String>,

    #[arg(
        short,
        long,
        help = "Number of repeat iterations (default from config: 5-10)"
    )]
    iterations: Option<usize>,
}

#[derive(Args, Debug)]
struct ImpactArgs {
    #[arg(long, help = "Base snapshot tree hash to diff against")]
    snapshot: Option<String>,

    #[arg(
        short,
        long,
        default_value_t = 3,
        help = "Maximum transitive depth to traverse"
    )]
    depth: usize,
}

#[derive(Args, Debug)]
struct TestArgs {
    #[arg(long, help = "Override framework (cargo_test, pytest, jest, go_test)")]
    framework: Option<String>,

    #[arg(short, long, help = "Filter pattern for test names")]
    filter: Option<String>,
}

#[derive(Args, Debug)]
struct InitArgs {
    #[arg(long, help = "Reinitialize an already-initialized project")]
    force: bool,

    #[arg(long, help = "Override auto-detected languages (repeatable)")]
    language: Option<Vec<String>>,
}

#[derive(Args, Debug)]
struct DoctorArgs {
    #[arg(long, help = "Limit to specific categories (repeatable)")]
    category: Option<Vec<String>>,
}

#[derive(Args, Debug)]
struct RunArgs {
    #[arg(
        long,
        help = "Override agent identifier (claude, aider, opencode, generic)"
    )]
    agent: Option<String>,

    #[arg(long, help = "Skip pre-run Git snapshot")]
    no_snapshot: bool,

    #[arg(
        last = true,
        required = true,
        help = "Agent command and arguments to execute"
    )]
    agent_command: Vec<String>,
}

#[derive(Args, Debug)]
struct SessionArgs {
    #[command(subcommand)]
    subcommand: SessionSubcommand,
}

#[derive(Subcommand, Debug)]
enum SessionSubcommand {
    #[command(about = "List recent agent sessions")]
    List {
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Args, Debug)]
struct TraceArgs {
    #[arg(long, help = "Filter by session ID")]
    session: Option<String>,

    #[arg(long, help = "Filter by event kind")]
    kind: Option<String>,
}

#[derive(Args, Debug)]
struct SnapshotArgs {
    #[command(subcommand)]
    subcommand: SnapshotSubcommand,
}

#[derive(Subcommand, Debug)]
enum SnapshotSubcommand {
    #[command(about = "List captured snapshots")]
    List,
}

#[derive(Args, Debug)]
struct RollbackArgs {
    #[arg(long, help = "Target snapshot tree hash or ID")]
    snapshot: Option<String>,

    #[arg(long, help = "Session ID to scope touched files")]
    session: Option<String>,

    #[arg(long, help = "Show what would be restored without modifying files")]
    dry_run: bool,

    #[arg(long, help = "Skip confirmation prompt")]
    yes: bool,

    #[arg(long, help = "Widen rollback scope to full diff (Forced mode)")]
    force: bool,

    #[arg(long, help = "Display past rollback audit history")]
    history: bool,
}

#[derive(Args, Debug)]
struct DiffArgs {
    #[arg(long, required = true, help = "Target snapshot tree hash")]
    snapshot: String,
}

#[derive(Args, Debug)]
struct ConfigArgs {
    #[command(subcommand)]
    subcommand: ConfigSubcommand,
}

#[derive(Subcommand, Debug)]
enum ConfigSubcommand {
    #[command(about = "Show merged effective configuration")]
    Show,

    #[command(about = "Validate configuration file syntax and patterns")]
    Validate,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.no_color {
        colored::control::set_override(false);
    }

    if cli.verbose {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let project_root = cli.project.unwrap_or_else(find_project_root);
    let engine = CoreEngine::new(&project_root);

    let exit_code = match execute_command(cli.command, &engine, cli.json, cli.quiet).await {
        Ok(code) => code,
        Err(err) => {
            if cli.json {
                let error_payload = serde_json::json!({
                    "error": err.to_string(),
                    "exit_code": err.exit_code()
                });
                eprintln!("{}", serde_json::to_string_pretty(&error_payload).unwrap());
            } else {
                eprintln!("{} {}", "error:".red().bold(), err);
            }
            err.exit_code()
        }
    };

    process::exit(exit_code);
}

async fn execute_command(
    command: Commands,
    engine: &CoreEngine,
    json: bool,
    quiet: bool,
) -> Result<i32, DavrError> {
    match command {
        Commands::Init(args) => {
            let config_path = engine.init(args.force, args.language)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "initialized",
                        "config_path": config_path.to_string_lossy(),
                        "project_root": engine.project_root().to_string_lossy()
                    })
                );
            } else if !quiet {
                println!(
                    "{} Initialized DAVR in {}",
                    "✔".green().bold(),
                    engine.project_root().display().to_string().bold()
                );
                println!("  Config: {}", config_path.display());
                println!(
                    "  Database: {}",
                    engine.project_root().join(".davr/davr.db").display()
                );
            }
            Ok(0)
        }

        Commands::Doctor(_args) => {
            let results = engine.doctor(None)?;
            let has_failures = results.iter().any(|r| r.status == CheckStatus::Fail);
            let has_warnings = results.iter().any(|r| r.status == CheckStatus::Warn);

            if json {
                println!("{}", serde_json::to_string_pretty(&results).unwrap());
            } else if !quiet {
                println!("\n{}", "DAVR Environment Doctor".bold().underline());
                println!("Target project: {}\n", engine.project_root().display());

                for result in &results {
                    let badge = match result.status {
                        CheckStatus::Pass => "[PASS]".green().bold(),
                        CheckStatus::Warn => "[WARN]".yellow().bold(),
                        CheckStatus::Fail => "[FAIL]".red().bold(),
                        CheckStatus::Skipped => "[SKIP]".dimmed(),
                    };
                    println!("{badge} {:<30} {}", result.name.bold(), result.detail);
                }

                println!();
                if has_failures {
                    println!("{}", "✖ Pre-flight checks failed. Fix issues above before running agent sessions.".red().bold());
                } else if has_warnings {
                    println!("{}", "⚠ Environment passed with warnings.".yellow().bold());
                } else {
                    println!("{}", "✔ All environment checks passed.".green().bold());
                }
            }

            if has_failures {
                Ok(2)
            } else if has_warnings {
                Ok(1)
            } else {
                Ok(0)
            }
        }

        Commands::Run(args) => {
            let cmd = args
                .agent_command
                .first()
                .ok_or_else(|| DavrError::Config("Missing agent command to execute".into()))?;
            let rest_args = &args.agent_command[1..];

            if !quiet && !json {
                println!(
                    "{} Starting supervised agent session for `{}`...",
                    "▶".cyan().bold(),
                    cmd.bold()
                );
            }

            let summary = engine
                .run_agent_session(args.agent.as_deref(), cmd, rest_args, args.no_snapshot)
                .await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&summary).unwrap());
            } else if !quiet {
                println!("\n{}", "DAVR Session Summary".bold().underline());
                println!("  Session ID:      {}", summary.session_id);
                println!("  Status:          {}", summary.status.bold());
                println!("  Exit Code:       {}", summary.exit_code);
                println!("  Duration:        {}ms", summary.duration_ms);
                if let Some(snap) = summary.pre_snapshot_id {
                    println!("  Pre-Run Snapshot: {}", snap.dimmed());
                }
                println!("  Files Modified:  {}", summary.files_changed.len());
                for f in summary.files_changed {
                    println!("    - {}", f);
                }
            }

            Ok(summary.exit_code)
        }

        Commands::Session(args) => match args.subcommand {
            SessionSubcommand::List { limit } => {
                let sessions = engine.list_sessions(limit)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&sessions).unwrap());
                } else {
                    println!("\n{}", "Recent Agent Sessions".bold().underline());
                    for s in sessions {
                        println!(
                            "  {:<36} {:<10} {:<10} {}",
                            s.id.dimmed(),
                            s.agent_name.bold(),
                            s.status,
                            s.command_line
                        );
                    }
                }
                Ok(0)
            }
        },

        Commands::Trace(args) => {
            let items = engine.get_trace(args.session.as_deref(), args.kind.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&items).unwrap());
            } else {
                println!("\n{}", "Telemetry Event Trace".bold().underline());
                for item in items {
                    println!(
                        "  [{}] {:<22} (table: {:?})",
                        item.severity,
                        item.kind.bold(),
                        item.ref_table.unwrap_or_else(|| "none".into())
                    );
                }
            }
            Ok(0)
        }

        Commands::Snapshot(args) => match args.subcommand {
            SnapshotSubcommand::List => {
                let snapshots = engine.list_snapshots()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&snapshots).unwrap());
                } else {
                    println!("\n{}", "Git Snapshots".bold().underline());
                    for snap in snapshots {
                        println!(
                            "  {:<36} tree: {:<12} reason: {:<14} dirty_before: {}",
                            snap.id,
                            &snap.tree_hash[..10],
                            snap.reason,
                            snap.dirty_before
                        );
                    }
                }
                Ok(0)
            }
        },

        Commands::Rollback(args) => {
            if args.history {
                let records = engine.list_rollbacks(20)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&records).unwrap());
                } else if !quiet {
                    println!("\n{}", "DAVR Rollback Audit History".bold().underline());
                    if records.is_empty() {
                        println!("  No rollback operations recorded yet.");
                    } else {
                        for r in records {
                            let status_badge = match r.status.as_str() {
                                "succeeded" => "[SUCCESS]".green().bold(),
                                "failed" => "[FAILED]".red().bold(),
                                _ => "[PARTIAL]".yellow().bold(),
                            };
                            println!(
                                "  {} Rollback: {} (Snapshot: {:.8}..., Restored: {}, Time: {})",
                                status_badge,
                                r.id,
                                r.snapshot_id,
                                r.files_restored_count,
                                chrono::DateTime::from_timestamp_millis(r.initiated_at)
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                    .unwrap_or_default()
                            );
                            if let Some(ref err) = r.error_message {
                                println!("      {}", err.red());
                            }
                        }
                    }
                    println!();
                }
                return Ok(0);
            }

            let scope = if args.force {
                RollbackScope::Forced
            } else {
                RollbackScope::SessionIntersection
            };

            let report = engine.rollback(
                args.snapshot.as_deref(),
                args.session.as_deref(),
                scope,
                args.dry_run,
                args.force,
            )?;

            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!("\n{}", "DAVR Rollback Operation".bold().underline());
                println!("  Target Snapshot: {}", report.snapshot_id);
                println!("  Status:          {}", report.status.bold());
                println!("  Scope:           {:?}", report.scope);
                println!("  Restored Files:  {}", report.restored_files.len());
                for f in &report.restored_files {
                    println!("    ✔ {}", f.green());
                }
                if !report.deleted_files.is_empty() {
                    println!(
                        "  Deleted Files (Agent Created): {}",
                        report.deleted_files.len()
                    );
                    for f in &report.deleted_files {
                        println!("    ✘ {}", f.red());
                    }
                }
                if !report.conflicted_files.is_empty() {
                    println!("\n  {}", "CONFLICTS DETECTED (Preserved):".yellow().bold());
                    for c in &report.conflicted_files {
                        println!("    ! {}", c.file_path.bold().yellow());
                        println!("      Reason: {}", c.reason.dimmed());
                    }
                    println!("  (Use {} to overwrite conflicted files)", "--force".bold());
                }
                if !report.excluded_files.is_empty() {
                    println!(
                        "  Excluded (Not in Session): {}",
                        report.excluded_files.len()
                    );
                    for f in &report.excluded_files {
                        println!("    - {} (unrelated developer edit)", f.dimmed());
                    }
                }
                if args.dry_run {
                    println!(
                        "\n{}",
                        "Dry-run complete. No files were modified.".cyan().bold()
                    );
                } else if report.status == "succeeded" {
                    println!("\n{}", "✔ Rollback completed successfully.".green().bold());
                } else {
                    println!(
                        "\n{}",
                        "⚠ Rollback finished with warnings/conflicts."
                            .yellow()
                            .bold()
                    );
                }
            }
            Ok(0)
        }

        Commands::Diff(args) => {
            let diffs = engine.diff_snapshot(&args.snapshot)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&diffs).unwrap());
            } else {
                println!(
                    "\n{}",
                    format!("Diff vs Snapshot {}", args.snapshot)
                        .bold()
                        .underline()
                );
                for d in diffs {
                    println!("  [{:<8}] {}", d.change_type, d.file_path);
                }
            }
            Ok(0)
        }

        Commands::Test(args) => {
            let results = engine
                .run_tests(args.framework.as_deref(), args.filter.as_deref())
                .await?;

            let all_passed = results.iter().all(|r| r.failed == 0 && r.exit_code == 0);

            if json {
                println!("{}", serde_json::to_string_pretty(&results).unwrap());
            } else if !quiet {
                println!("\n{}", "DAVR Test Execution Summary".bold().underline());
                for suite in &results {
                    let badge = if suite.failed == 0 && suite.exit_code == 0 {
                        "[PASS]".green().bold()
                    } else {
                        "[FAIL]".red().bold()
                    };
                    println!(
                        "{} {:<12} ({} passed, {} failed, {} skipped, {}ms)",
                        badge,
                        suite.framework.bold(),
                        suite.passed,
                        suite.failed,
                        suite.skipped,
                        suite.duration_ms
                    );

                    for tc in &suite.test_cases {
                        match tc.status {
                            davr_core::TestCaseStatus::Passed => {
                                println!("    {} {}", "✓".green(), tc.name.dimmed());
                            }
                            davr_core::TestCaseStatus::Failed => {
                                println!("    {} {}", "✗".red().bold(), tc.name.bold());
                                if let Some(ref err) = tc.error_message {
                                    println!("      {}", err.red());
                                }
                            }
                            davr_core::TestCaseStatus::Skipped => {
                                println!("    {} {}", "○".yellow(), tc.name.dimmed());
                            }
                            _ => {
                                println!("    {} {}", "!".red(), tc.name);
                            }
                        }
                    }
                }
                println!();
            }

            if all_passed {
                Ok(0)
            } else {
                Ok(1)
            }
        }

        Commands::Analyze => {
            let summary = engine.analyze_project()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary).unwrap());
            } else if !quiet {
                println!("\n{}", "DAVR AST Codebase Analysis".bold().underline());
                println!("  Target Project:     {}", engine.project_root().display());
                println!(
                    "  Source Files:       {}",
                    summary.files_indexed.to_string().bold()
                );
                println!(
                    "  Symbols Extracted:  {}",
                    summary.symbols_extracted.to_string().cyan().bold()
                );
                println!(
                    "  Dependency Edges:   {}",
                    summary.dependency_edges.to_string().green().bold()
                );
                println!(
                    "\n{}",
                    "✔ AST index and dependency graph updated.".green().bold()
                );
            }
            Ok(0)
        }

        Commands::Impact(args) => {
            let report = engine.analyze_impact(args.snapshot.as_deref(), args.depth)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else if !quiet {
                println!(
                    "\n{}",
                    "DAVR Transitive Change Impact Analysis".bold().underline()
                );
                println!(
                    "  Directly Changed Files:  {}",
                    report.directly_modified_files.len()
                );
                for f in &report.directly_modified_files {
                    println!("    • {}", f.cyan());
                }

                println!(
                    "\n  Impacted Source Files (Transitive Closure): {}",
                    report.impacted_source_files.len()
                );
                for f in &report.impacted_source_files {
                    println!(
                        "    + {:<35} (depth: {}, confidence: {:?}, reason: {})",
                        f.file_path.bold(),
                        f.depth,
                        f.confidence,
                        f.reason.dimmed()
                    );
                }

                println!(
                    "\n  Impacted Tests Selected: {}",
                    report.impacted_tests.len()
                );
                for t in &report.impacted_tests {
                    println!("    🎯 {}", t.test_file.green().bold());
                }
                println!();
            }
            Ok(0)
        }

        Commands::Flaky(args) => {
            let report = engine
                .run_flaky_tests(
                    args.framework.as_deref(),
                    args.filter.as_deref(),
                    args.iterations,
                )
                .await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else if !quiet {
                println!("\n{}", "DAVR Flakiness Test Analysis".bold().underline());
                println!("  Total Unique Tests:  {}", report.total_tests);
                println!(
                    "  Stable Pass:         {}",
                    report.stable_pass.to_string().green().bold()
                );
                println!(
                    "  Stable Fail:         {}",
                    report.stable_fail.to_string().red().bold()
                );
                println!(
                    "  Flaky Detected:      {}",
                    report.flaky_detected.to_string().yellow().bold()
                );
                println!(
                    "  Timeout Unstable:    {}",
                    report.timeout_unstable.to_string().purple().bold()
                );

                println!("\n{}", "Individual Test Stability Classification:".bold());
                for c in &report.reports {
                    let badge = match c.classification {
                        davr_core::FlakyClassification::StablePass => "[STABLE_PASS]".green(),
                        davr_core::FlakyClassification::StableFail => "[STABLE_FAIL]".red(),
                        davr_core::FlakyClassification::Flaky => "[FLAKY]".yellow().bold(),
                        davr_core::FlakyClassification::TimeoutUnstable => {
                            "[TIMEOUT_UNSTABLE]".purple().bold()
                        }
                        _ => "[UNKNOWN]".dimmed(),
                    };
                    println!(
                        "  {} {:<35} (passed: {}/{}, rate: {:.0}%)",
                        badge,
                        c.test_name,
                        c.pass_count,
                        c.iterations_run,
                        c.pass_rate * 100.0
                    );
                }
                println!();
            }

            if report.flaky_detected == 0 {
                Ok(0)
            } else {
                Ok(1)
            }
        }

        Commands::Mcp => {
            let server = davr_mcp::McpServer::new(engine.project_root());
            server.run_stdio().await?;
            Ok(0)
        }

        Commands::Config(args) => match args.subcommand {
            ConfigSubcommand::Show => {
                let config = Config::load_from_dir(engine.project_root())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&config).unwrap());
                } else {
                    println!("{}", config.to_toml_string()?);
                }
                Ok(0)
            }
            ConfigSubcommand::Validate => {
                let config = Config::load_from_dir(engine.project_root())?;
                config.validate()?;
                if json {
                    println!("{}", serde_json::json!({ "status": "valid" }));
                } else if !quiet {
                    println!("{} Configuration is valid", "✔".green().bold());
                }
                Ok(0)
            }
        },

        Commands::Version => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "name": "davr",
                        "version": "0.1.0",
                        "rustc": option_env!("RUSTC_VERSION_SUMMARY").unwrap_or("stable"),
                        "sqlite": "bundled (WAL mode)",
                    })
                );
            } else {
                println!("{} {}", "davr".bold(), "0.1.0");
                println!("  SQLite: bundled (WAL mode)");
                println!("  Architecture: Deterministic Agent Verification Runtime");
            }
            Ok(0)
        }
    }
}
