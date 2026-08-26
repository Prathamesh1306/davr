# DAVR — Deterministic Agent Verification Runtime

<div align="center">

**AI generates. DAVR verifies.**

*A local safety supervisor and deterministic verification runtime for AI coding agents.*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 2021 Edition](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Built with: libgit2](https://img.shields.io/badge/Git-libgit2-green.svg)](https://libgit2.org/)
[![Storage: SQLite WAL](https://img.shields.io/badge/Database-SQLite_WAL-blueviolet.svg)](https://www.sqlite.org/)
[![CI](https://img.shields.io/badge/Build-Passing-brightgreen.svg)](#testing)

</div>

---

## ⚡ What is DAVR?

**DAVR** (pronounced *day-ver*) is a local CLI safety runtime designed to supervise and verify autonomous AI coding agents (such as Claude Code, Aider, OpenCode, Cline, or custom agent scripts).

### The Problem
When autonomous AI agents generate code and execute tasks in your terminal:
1. **Silent Overwrites:** An agent can unintentionally modify or delete your uncommitted manual work.
2. **Execution Failures:** Tasks fail midway due to missing runtime tools, missing environment variables, or dirty repository states.
3. **Destructive Commands:** Malformed commands (e.g., unintended deletions or database drops) can execute before human intervention.
4. **Credential Leaks:** Raw API keys and tokens passed via command-line arguments can get written into plain text terminal and session logs.
5. **Orphaned Processes:** Uncontrolled background processes can remain running after agent crashes, locking ports and workspace resources.

### The Solution
DAVR wraps agent execution in an isolated supervision harness. Before an agent runs, DAVR validates environment dependencies, captures a lightweight Git object-database (ODB) tree snapshot, evaluates top-level commands against security policy rules, and tracks filesystem mutations. If changes must be reverted, DAVR provides **conflict-aware, crash-recoverable transactional rollbacks** that protect your independent developer edits.

---

## 🛡️ Core Safety Model & Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                 DAVR CLI                                    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                ┌──────────────────────┼──────────────────────┐
                ▼                      ▼                      ▼
      ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
      │ Environment Check│   │ Security Policy  │   │ Git ODB Snapshot │
      │  (davr-env)      │   │  (davr-security) │   │  (davr-git)      │
      └─────────┬────────┘   └─────────┬────────┘   └─────────┬────────┘
                │                      │                      │
                └──────────────────────┼──────────────────────┘
                                       │
                                       ▼
                     ┌──────────────────────────────────┐
                     │   Supervised Process Runtime     │
                     │   (davr-agent + davr-fs)         │
                     │   - Process group isolation      │
                     │   - Environment sanitization     │
                     │   - Debounced BLAKE3 hash watch  │
                     └─────────────────┬────────────────┘
                                       │
                                       ▼
                     ┌──────────────────────────────────┐
                     │    Post-Session State Capture    │
                     │    - Hash map of touched files   │
                     │    - SQLite WAL state log        │
                     └─────────────────┬────────────────┘
                                       │
                                       ▼
                     ┌──────────────────────────────────┐
                     │  3-Way Conflict-Aware Rollback   │
                     │  (RollbackPlanner + Journal)     │
                     │  - State comparison (A ∩ B)      │
                     │  - Pre-rollback backup journal   │
                     └──────────────────────────────────┘
```

### 1. 3-Way State Conflict Detection ($A \to B \to C$)
DAVR tracks three distinct file states to prevent rolling back changes made by human developers after an agent finishes:
- **$A$ (Snapshot State):** Repository file content before the agent begins.
- **$B$ (Post-Agent State):** Content hash captured immediately when the agent process exits.
- **$C$ (Current Working Tree State):** On-disk content at the moment `davr rollback` is executed.

$$\begin{aligned}
\text{If } C = B &\implies \textbf{Safe to Rollback} \quad (\text{Restores } A \text{ or deletes agent-created file}) \\
\text{If } C \neq B &\implies \textbf{Conflict Detected!} \quad (\text{Preserves } C \text{ by default; requires } \texttt{--force})
\end{aligned}$$

| Scenario | Snapshot ($A$) | Post-Agent ($B$) | Current ($C$) | Result |
|---|---|---|---|---|
| **Agent modified tracked file** | $A_1$ | $B_1$ | $B_1$ | 🔄 **Restored to $A_1$** |
| **Developer modified file after agent** | $A_1$ | $B_1$ | $C_1$ | ⚠️ **Conflict: $C_1$ preserved** |
| **Agent created new file** | Missing | $B_1$ | $B_1$ | 🗑️ **Deleted** |
| **Developer modified agent's new file** | Missing | $B_1$ | $C_1$ | ⚠️ **Conflict: $C_1$ preserved** |
| **Unrelated developer edit** | $A_1$ | *Not in session* | $C_1$ | 🛡️ **Excluded from rollback** |

### 2. Transactional Rollback Journal
Rollback operations execute through an atomic journal staged in `.davr/rollback-txn/<id>/`:
$$\text{PREPARED} \longrightarrow \text{BACKED\_UP} \longrightarrow \text{APPLYING} \longrightarrow \text{COMMITTED} \text{ / } \text{ABORTED}$$

Before any file on disk is modified or deleted, DAVR copies the target file to the transaction backup store. If an I/O error or process interruption occurs during the apply phase, DAVR restores all staged backups, preventing partial or corrupt workspace states.

### 3. Symlink & Path Containment
All file paths targeted for rollback are validated using `validate_path_containment()`. Parent traversal components (`..`) and symlinks resolving outside the repository root are rejected, preventing symlink escape vulnerabilities.

### 4. Top-Level Policy Enforcement & Secret Redaction
- **Top-Level Command Guard:** Evaluates command strings passed to `davr run` against configured regex/glob patterns (`blocked_commands`, `confirm_commands`) before spawning processes. Blocked commands terminate with exit code `20`.
- **Secret Redaction:** High-entropy credential patterns (e.g., `sk-...`, `ghp_...`) are redacted before command strings are written to SQLite database records or emitted via telemetry.

### 5. Runtime Scope & Security Boundaries
> **Important Note on Sandboxing Boundaries:**  
> DAVR is a **process supervisor and Git/filesystem safety harness**, not an OS kernel sandbox.
> - **In Scope:** Top-level commands passed directly to `davr run`, environment sanitization, process group lifecycle management (`SIGINT`/`SIGTERM`/`SIGKILL`), filesystem event hashing, and Git snapshot rollbacks.
> - **Out of Scope / Limitation:** Commands spawned internally by third-party interactive shells or agent subprocesses are not intercepted via kernel-level hooks (such as seccomp, ptrace, or eBPF).

---

## 📥 Installation

### Prerequisites
- **Rust Toolchain:** Stable Rust (2021 edition, 1.80+ recommended)
- **Git:** Git 2.30+ installed and available on `PATH`
- **Platform:** macOS or Linux (Windows process group management is in development)

### Option 1: Cargo Install (From Source)
```bash
git clone https://github.com/Prathamesh1306/davr.git
cd davr
cargo install --path crates/davr-cli
```

### Option 2: Shell Installer
```bash
./install.sh
```

### Verify Installation
```bash
davr version
davr --help
```

---

## 🚀 Quick-Start Guide

### 1. Initialize DAVR in Your Project
```bash
cd /path/to/your/project
davr init
```
This detects project languages (Rust, TypeScript/JavaScript, Python, Go) and creates `.davr/config.toml` along with the local SQLite storage engine at `.davr/davr.db`.

### 2. Run Pre-Flight Health Checks
```bash
davr doctor
```
Verifies compiler availability, package manager lockfiles, Git repository status, required environment variables, and workspace write permissions.

### 3. Supervise an AI Agent Session
Prefix your agent command with `davr run --`:

```bash
# Supervise Claude Code
davr run -- claude "Refactor auth middleware to use JWT"

# Supervise Aider
davr run -- aider --message "Add unit tests for user service"

# Supervise a Python agent script
davr run -- python agent.py --task "Optimize query indexes"
```

**What DAVR does during execution:**
1. Validates pre-flight environment checks.
2. Checks top-level command against security policies.
3. Takes an instantaneous Git ODB tree snapshot (`refs/davr/snapshots/<id>`) without cluttering `git log`.
4. Spawns the agent in an isolated Unix process group with sanitized environment variables.
5. Monitors filesystem events and computes post-session file hashes.
6. Records redacted session metadata in `.davr/davr.db`.

### 4. Roll Back Agent Changes Safely
If an agent produces undesirable modifications:

```bash
# Preview what would be restored or deleted (dry run)
davr rollback --dry-run

# Execute the rollback
davr rollback --yes

# View rollback audit log
davr rollback --history
```

If developer edits occurred after the agent ran, DAVR flags conflicts and keeps developer work intact:
```text
  CONFLICTS DETECTED (Preserved):
    ! src/auth.rs
      Reason: File content modified after agent session.
  (Use --force to overwrite conflicted files)
```

---

## 📋 CLI Reference

| Command | Usage | Description |
|---|---|---|
| `init` | `davr init [--force] [--language <lang...>]` | Initialize `.davr/` workspace configuration and SQLite database |
| `doctor` | `davr doctor [--category <cat...>]` | Run environment, runtime, tool, and permission checks |
| `run` | `davr run [--agent <id>] [--no-snapshot] -- <command...>` | Wrap and supervise an agent process with snapshotting and telemetry |
| `rollback` | `davr rollback [--dry-run] [--yes] [--force] [--history] [--snapshot <hash>] [--session <id>]` | Perform 3-way conflict-aware rollback to prior snapshot |
| `session` | `davr session list [--limit <n>]` | List recent agent execution sessions and exit statuses |
| `trace` | `davr trace [--session <id>] [--kind <kind>]` | Inspect structured telemetry event streams from SQLite |
| `snapshot` | `davr snapshot list` | List captured Git tree snapshot hashes and timestamps |
| `diff` | `davr diff --snapshot <hash>` | Show file differences between a snapshot and the working tree |
| `test` | `davr test [--framework <name>] [--filter <pattern>]` | Execute multi-framework test runner (Cargo, Pytest, Jest, Go) |
| `analyze` | `davr analyze` | Index source code symbols and build local dependency graph |
| `impact` | `davr impact [--depth <n>] [--snapshot <hash>]` | Run transitive change impact analysis on modified files |
| `flaky` | `davr flaky [--framework <name>] [--filter <p>] [--iterations <n>]` | Run repeat test stress analysis to classify flakiness |
| `mcp` | `davr mcp` | Launch Model Context Protocol (MCP) stdio JSON-RPC 2.0 server |
| `config` | `davr config show` / `davr config validate` | Display merged configuration or validate pattern syntax |
| `version` | `davr version` | Print version and compilation metadata |

### Global Flags
- `--project <path>`: Override target workspace root directory.
- `--config <path>`: Specify an explicit configuration file.
- `--json`: Format command output as structured JSON.
- `--no-color`: Disable ANSI color formatting in output.
- `--quiet`: Suppress non-essential diagnostic output.
- `--verbose`: Enable detailed diagnostic logging via `tracing`.

---

## ⚙️ Configuration (`.davr/config.toml`)

```toml
[project]
name = "my-project"
languages = ["rust", "typescript"]

[environment]
required_env_vars = ["ANTHROPIC_API_KEY"]
required_credentials = []
docker_required = false
warnings_block_run = false

[[environment.required_tools]]
name = "node"
min_version = "18.0.0"

[[environment.required_tools]]
name = "git"
min_version = "2.30.0"

[agent]
default_agent = "generic"
allowed_agents = ["claude", "aider", "opencode", "generic"]
timeout_seconds = 1800
sanitize_env = true
env_allowlist = ["PATH", "HOME", "LANG", "ANTHROPIC_API_KEY"]

[security]
blocked_commands = [
    'regex:^rm\s+-rf\s+/(\s|$)',
    "glob:git push --force*"
]
confirm_commands = [
    "glob:git push*",
    "glob:*DROP TABLE*"
]
redact_patterns = [
    "regex:sk-[A-Za-z0-9]{20,}",
    "regex:ghp_[A-Za-z0-9]{36}"
]

[git]
snapshot_on_run = true
max_snapshots_per_project = 20
snapshot_retention_days = 14

[telemetry]
enabled = true
retention_days = 30
verification_retention_days = 90

[test]
frameworks = ["cargo_test", "pytest", "jest", "go_test"]
impact_min_confidence = "medium"
fallback_to_full_suite = true
parallelism = 0

[flaky]
iterations = 10
timeout_seconds = 30
max_parallel_iterations = 4

[mcp]
enabled = false
allow_mutating_tools = false
```

---

## 🏗️ Repository Architecture

DAVR is organized into 16 focused crates:

| Crate | Responsibility |
|---|---|
| [`davr-types`](crates/davr-types) | Domain identifiers (`SessionId`, `SnapshotId`), `FileState` enum, error taxonomy, and standard exit codes. |
| [`davr-storage`](crates/davr-storage) | SQLite WAL engine, database schema migrations, and query operations. |
| [`davr-config`](crates/davr-config) | Configuration parser, environment override resolution, and regex validation. |
| [`davr-security`](crates/davr-security) | Policy evaluation engine (`evaluate_command`) and regex-based secret redactor. |
| [`davr-env`](crates/davr-env) | Pre-flight validation checks with language adapters for Rust, TypeScript, Python, and Go. |
| [`davr-fs`](crates/davr-fs) | Debounced filesystem watcher with BLAKE3 hash generation. |
| [`davr-git`](crates/davr-git) | Pure `RollbackPlanner`, transactional `RollbackExecutor`, and Git snapshot management via `libgit2`. |
| [`davr-agent`](crates/davr-agent) | Process supervisor with Unix process group isolation and timeout signal handling. |
| [`davr-telemetry`](crates/davr-telemetry) | Batched event emitter (50 events / 200ms flush threshold). |
| [`davr-ast`](crates/davr-ast) | Regex-based source symbol extractor and import relationship parser. |
| [`davr-impact`](crates/davr-impact) | Breadth-First Search (BFS) blast radius dependency analyzer. |
| [`davr-test`](crates/davr-test) | Multi-framework test execution harness (Cargo, Pytest, Jest, Go). |
| [`davr-flaky`](crates/davr-flaky) | Repeat test runner with statistical flakiness classifier. |
| [`davr-mcp`](crates/davr-mcp) | Stdio JSON-RPC 2.0 Model Context Protocol server. |
| [`davr-core`](crates/davr-core) | Core orchestration engine coordinating all subsystems. |
| [`davr-cli`](crates/davr-cli) | Main binary entry point providing the user CLI interface. |

---

## 🧪 Testing

Run the full test suite across all workspace crates:

```bash
# Check compilation across all crates
cargo check --workspace

# Run all unit and integration tests
cargo test --workspace

# Check formatting
cargo fmt --all -- --check

# Run linter
cargo clippy --workspace --all-targets
```

---

## 🗺️ Roadmap

- [ ] **Tree-sitter AST Integration:** Replace regex-based parsing with robust tree-sitter grammars across supported languages.
- [ ] **Windows Process Containment:** Implement Windows Job Object hierarchy for cross-platform process tree supervision.
- [ ] **Extended MCP Capabilities:** Add dynamic resource subscriptions and automated diagnostic prompts for IDEs.
- [ ] **Active Session Snapshot Guard:** Enhance retention pruning to explicitly protect running session snapshots under high concurrency.

---

## 🤝 Contributing

Contributions are welcome! Please review [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before submitting pull requests.

---

## 🔒 Security

For security vulnerability reporting and details on our threat model and boundaries, please see [SECURITY.md](SECURITY.md).

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
