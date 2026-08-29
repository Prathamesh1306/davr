# DAVR System Architecture

DAVR (**Deterministic Agent Verification Runtime**) is a deterministic verification runtime designed to supervise autonomous AI coding agents, track all workspace mutations, enforce security boundaries, execute impact-selected test suites, and provide conflict-free rollback mechanisms.

## Core Philosophy

> **"AI generates. DAVR verifies."**

Autonomous agents can rapidly produce code, but their edits may introduce silent syntax errors, flaky test failures, policy-violating shell executions, or unwanted modifications to human developer code. DAVR wraps agent processes with a deterministic verification harness.

---

## High-Level Architecture

```
                       +-------------------------------+
                       |           DAVR CLI            |
                       |       (davr-cli / MCP)        |
                       +---------------+---------------+
                                       |
                               +-------v-------+
                               |  Core Engine  |
                               |  (davr-core)  |
                               +-------+-------+
                                       |
     +-----------------+---------------+---------------+-----------------+
     |                 |               |               |                 |
+----v----+       +----v----+     +----v----+     +----v----+       +----v----+
| Agent   |       | Security|     | Git &   |     | AST &   |       | Test &  |
| Runtime |       | Engine  |     | Rollback|     | Impact  |       | Flaky   |
+----+----+       +----+----+     +----+----+     +----+----+       +----+----+
     |                 |               |               |                 |
     +-----------------+---------------+---------------+-----------------+
                                       |
                       +---------------+---------------+
                       |      SQLite State Store       |
                       |  (davr-storage / Telemetry)   |
                       +-------------------------------+
```

---

## Subsystems

### 1. Agent Runtime (`davr-agent`)
- Spawns agent processes (e.g. Claude Code, Aider, OpenCode, or custom generic binaries).
- Isolates processes into process groups (Unix `setpgid`) and Job Objects / process trees (Windows `taskkill /F /T`).
- Enforces strict process timeouts and handles Ctrl+C graceful abort sequences.
- Sanitizes environment variables via `env_allowlist`.

### 2. Security Engine (`davr-security`)
- Evaluates commands **pre-spawn** against regex and glob blocklists/confirmlists.
- Rejects destructive operations (e.g. `rm -rf /`, `git push --force`) with exit code `20`.
- Redacts API keys, bearer tokens, and credentials before persistence in SQLite, telemetry, and logs.

### 3. Git & Rollback Engine (`davr-git`)
- Takes lightweight Git tree snapshots before and after agent runs without mutating working branches.
- Implements **Intersection Rollback** ($A \cap B$):
  - Only reverts files modified by the specific agent session.
  - Detects developer edits made during or after the session ($C \neq B$) and protects them as conflicts unless `--force` is provided.
  - Executes atomic rollback via a write-ahead journal (`PREPARED` $\to$ `BACKED_UP` $\to$ `APPLYING` $\to$ `COMMITTED`).

### 4. AST & Impact Engine (`davr-ast`, `davr-impact`)
- Incremental parsing using **Tree-sitter** for Rust, TypeScript/JavaScript, Python, and Go.
- Tolerates partial or broken syntax during active agent edits, extracting valid AST nodes and flagging files with `parse_incomplete = true`.
- Resolves cross-file symbol references and import dependency graphs.
- Computes transitive change impact graphs to select affected test cases.

### 5. Test & Flaky Verification (`davr-test`, `davr-flaky`)
- Discovers and executes tests across Cargo, Pytest, Jest, and Go Test.
- Classifies flakiness across repeat iterations (`STABLE_PASS`, `STABLE_FAIL`, `FLAKY`, `TIMEOUT_UNSTABLE`).

### 6. Storage & Telemetry (`davr-storage`, `davr-telemetry`)
- Embedded SQLite in WAL mode (`.davr/davr.db`) with 15 normalized relational tables.
- Comprehensive telemetry event audit stream (`SESSION_STARTED`, `COMMAND_STARTED`, `VERIFICATION_STARTED`, `ROLLBACK_APPLIED`, etc.).
