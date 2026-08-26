# Changelog

All notable changes to DAVR will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.0] - 2026-08-27

### Added
- **3-Way State Conflict Detection ($A \to B \to C$):** Pure `RollbackPlanner` computes states across pre-session snapshot ($A$), post-agent execution ($B$), and current working tree ($C$). Automatically detects conflicts when $C \neq B$, preventing unintended overwrites of human developer modifications.
- **Transactional Rollback Journal (`RollbackExecutor`):** Implemented crash-recoverable rollback journal staging operations in `.davr/rollback-txn/<id>/` with explicit states (`PREPARED` $\to$ `BACKED_UP` $\to$ `APPLYING` $\to$ `COMMITTED`/`ABORTED`). Automatic restoration from backups on I/O error or interruption during apply.
- **Symlink & Path Containment Guard:** Added `validate_path_containment()` in `davr-git` to reject parent directory traversals (`..`) and symlinks resolving outside the repository root.
- **Pre-Spawn Security Policy Enforcement:** Wired `SecurityEngine::evaluate_command()` into `CoreEngine::run_agent_session()` to reject blocked commands prior to process execution (terminating with exit code `20`).
- **Secret Redaction Pipeline:** Added automated regex-based secret token redaction (`sk-...`, `ghp_...`) before command lines are persisted into SQLite `agent_sessions` or emitted via telemetry.
- **Rollback Audit Logging (`davr rollback --history`):** Added SQLite persistence of all rollback operations (`rollback_operations` table) and CLI display flag.
- **Snapshot Retention Pruning:** Implemented `GitManager::prune_old_snapshots()` to prune Git references (`refs/davr/snapshots/<id>`) and SQLite snapshot records exceeding `max_snapshots_per_project`.
- **Safety Hardening Matrix Test Suite:** Added comprehensive matrix integration tests (`crates/davr-core/tests/safety_hardening_matrix_test.rs`) covering concurrent edit conflicts, developer-modified agent files, secret redaction, policy blocking, and rollback audit logs.

### Changed
- Refactored `davr rollback` CLI to support `--dry-run`, `--force`, `--yes`, and `--history` flags.
- Updated `CoreEngine::rollback()` to use `RollbackPlanner` and `RollbackExecutor`.

---

## [0.1.0] - 2026-08-20

### Added
- **Core CLI Subcommands:** `init`, `doctor`, `run`, `rollback`, `session`, `trace`, `snapshot`, `diff`, `test`, `analyze`, `impact`, `flaky`, `mcp`, `config`, `version`.
- **Environment Doctor (`davr-env`):** 7-category pre-flight environment checks across OS, PATH, runtimes, package managers, lockfiles, Git state, and write permissions for Rust, TypeScript/JavaScript, Python, and Go.
- **Git ODB Snapshotting (`davr-git`):** Content-addressed tree snapshots using `libgit2` with `refs/davr/snapshots/<id>` GC protection.
- **Filesystem Event Watcher (`davr-fs`):** Real-time filesystem mutation tracking with 300ms debouncing and BLAKE3 hash generation.
- **Agent Process Supervisor (`davr-agent`):** Subprocess management with Unix process group isolation (`setpgid`), environment variable sanitization, and timeout handling.
- **Unified Test Adapter (`davr-test`):** Output parsing and execution harness for Cargo, Pytest, Jest, and Go test runners.
- **Flakiness Stress Analyzer (`davr-flaky`):** Repeat-runner classification engine (`STABLE_PASS`, `STABLE_FAIL`, `FLAKY`, `TIMEOUT_UNSTABLE`).
- **Source AST Indexer & Impact Analyzer (`davr-ast`, `davr-impact`):** Regex-based symbol extraction and BFS blast radius dependency graph traversal.
- **Model Context Protocol Server (`davr-mcp`):** Stdio JSON-RPC 2.0 MCP server exposing doctor, test, impact, rollback, and session status tools.
- **SQLite Storage & Telemetry (`davr-storage`, `davr-telemetry`):** SQLite persistence in WAL mode with idempotent database migrations and batched event queueing.
