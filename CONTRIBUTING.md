# Contributing to DAVR

Thank you for your interest in contributing to DAVR! We welcome contributions from the community.

Please take a moment to review this document before submitting issues or pull requests.

---

## Code of Conduct

All contributors and participants are expected to adhere to our [Code of Conduct](CODE_OF_CONDUCT.md).

---

## Development Prerequisites

- **Rust:** Stable Rust (2021 edition, 1.80 or higher). Install via [rustup](https://rustup.rs/):
  ```bash
  rustup update stable
  rustup component add clippy rustfmt
  ```
- **Git:** Git 2.30 or newer.
- **Operating System:** macOS or Linux (POSIX environment for process groups).

---

## Workspace Structure

DAVR is organized as a Cargo workspace with 16 modular crates in [`crates/`](crates/):

```
crates/
├── davr-types        # Domain IDs, Error taxonomy, FileState enums, exit codes
├── davr-storage      # SQLite persistence, migrations, and query interfaces
├── davr-config       # TOML configuration loading and pattern validation
├── davr-security     # Security policy pattern evaluation & secret redaction
├── davr-env          # Pre-flight environment checks & language adapters
├── davr-fs           # Filesystem event monitoring & BLAKE3 hashing
├── davr-git          # Git ODB snapshots, RollbackPlanner, RollbackExecutor
├── davr-agent        # Process supervision, signal handling & process groups
├── davr-telemetry    # Batched SQLite telemetry emitter
├── davr-ast          # Source code symbol & import extraction
├── davr-impact       # Transitive BFS blast-radius change impact analysis
├── davr-test         # Unified test framework adapter (Cargo, Pytest, Jest, Go)
├── davr-flaky        # Repeat test runner for flakiness classification
├── davr-mcp          # Model Context Protocol (MCP) stdio JSON-RPC server
├── davr-core         # Top-level orchestration engine
└── davr-cli          # Multi-command CLI binary
```

---

## Build & Test Workflows

### Building
```bash
# Build all workspace crates in debug mode
cargo build --workspace

# Build optimized release binary
cargo build --release --workspace
```

### Running Tests
All changes must pass the full workspace test suite:
```bash
# Run all unit and integration tests
cargo test --workspace

# Run a specific test with full output
cargo test --package davr-core --test safety_hardening_matrix_test -- --nocapture
```

### Formatting & Linting
Ensure code meets style guidelines before submitting:
```bash
# Check formatting
cargo fmt --all -- --check

# Apply formatting automatically
cargo fmt --all

# Run Clippy lints
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Special Guidance for Critical Subsystems

### 1. Rollback & Git Engine (`davr-git`, `davr-core`)
- **3-Way State Invariants:** The `RollbackPlanner` must preserve human edits when $C \neq B$. Never bypass conflict checks unless the `force` flag is explicitly `true`.
- **Transactional Journaling:** `RollbackExecutor` must maintain the journal lifecycle (`PREPARED` $\to$ `BACKED_UP` $\to$ `APPLYING` $\to$ `COMMITTED`/`ABORTED`). If any write fails during apply, the recovery loop must restore all backed-up files.
- **Path Containment:** Always validate file paths with `validate_path_containment()` to block path traversals (`..`) and symlink escapes.
- **Testing:** Any change touching rollback logic must include tests in [`crates/davr-core/tests/safety_hardening_matrix_test.rs`](crates/davr-core/tests/safety_hardening_matrix_test.rs) or [`crates/davr-git/src/lib.rs`](crates/davr-git/src/lib.rs).

### 2. Security & Redaction (`davr-security`)
- **Pre-Spawn Checks:** Command policy evaluation must always execute *before* any child process is spawned.
- **Secret Redaction:** Ensure all newly introduced logs, telemetry payloads, or database fields redact high-entropy tokens.
- **Exit Codes:** Security violations must consistently return exit code `20` via `DavrError::Security`.

### 3. Database & Migrations (`davr-storage`)
- Database migrations in `migrations/` must remain idempotent and transactional.
- Always preserve `PRAGMA foreign_keys = ON` and `PRAGMA journal_mode = WAL`.

---

## Pull Request Guidelines

1. **Branch Naming:** Use descriptive branch names (e.g., `feature/tree-sitter-parser`, `fix/rollback-symlink-check`).
2. **Atomic Commits:** Keep commits logical and well-described.
3. **Tests Required:** Add unit tests for new functionality and integration tests for user-facing changes.
4. **PR Description:** Fill out the [Pull Request Template](.github/PULL_REQUEST_TEMPLATE.md) completely.
5. **No Breaking Changes Without Discussion:** Open an issue first before proposing major API or architectural changes.

---

## Issue Workflow

- **Bug Reports:** Use the [Bug Report template](.github/ISSUE_TEMPLATE/bug_report.yml). Include reproduction steps and environment information.
- **Feature Requests:** Use the [Feature Request template](.github/ISSUE_TEMPLATE/feature_request.yml). Explain the problem, proposed solution, and alternatives considered.
- **Security Vulnerabilities:** Follow the process in [SECURITY.md](SECURITY.md). Do **not** open public issues for security vulnerabilities.
