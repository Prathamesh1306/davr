# DAVR CLI Reference

## Global Options

All `davr` commands support these global flags:

| Flag | Description |
|---|---|
| `--project <PATH>` | Override the target project root directory |
| `--config <PATH>` | Specify an explicit `.davr/config.toml` path |
| `--json` | Emit machine-readable JSON output |
| `--no-color` | Disable ANSI color codes |
| `-q`, `--quiet` | Suppress non-essential output |
| `-v`, `--verbose` | Enable diagnostic debug logs |

---

## Exit Code Taxonomy

| Code | Meaning | Example |
|---|---|---|
| `0` | Success | All operations or verification passed |
| `1` | General Failure | Unhandled I/O error or failed sub-process |
| `2` | Configuration / Usage Error | Invalid CLI arguments or unparseable `config.toml` |
| `10–19` | Environment / Pre-flight Failure | Missing compiler, tool, or required environment variable |
| `20–29` | Security Policy Block | Command matched `blocked_commands` policy |
| `30–39` | Git / Snapshot / Rollback Error | Dirty working tree conflict or rollback journal failure |
| `40–49` | Database / Storage Error | SQLite query failure or migration conflict |
| `50+` | Agent Supervision Error | Agent process timed out or crashed |

---

## Command Catalog

### `davr init`
Initializes a `.davr/` directory with a SQLite state database, default `config.toml`, and `.gitignore` integration.
```bash
davr init
davr init --force
davr init --languages rust,python
```

### `davr doctor`
Performs comprehensive pre-flight verification: language toolchains, required binaries, env vars, git status, and database integrity.
```bash
davr doctor
```

### `davr status`
Displays high-level project summary: active branch, working tree dirty status, environment health check counters, latest session, and active snapshot.
```bash
davr status
```

### `davr run`
Wraps and supervises an autonomous AI coding agent session, capturing file mutations, logs, and telemetry.
```bash
davr run -- claude
davr run -- aider --model gpt-4o
```

### `davr exec`
Executes a single supervised command under security policy checking, secret redaction, and telemetry emission.
```bash
davr exec -- echo "hello"
davr exec -- rm -rf / # Exits with code 20 (blocked)
```

### `davr session`
Inspects recorded agent sessions and historical touched files.
```bash
davr session list
davr session show <SESSION_ID>
```

### `davr trace`
Displays the full chronological telemetry event stream for a session or the whole project.
```bash
davr trace
davr trace --session <SESSION_ID>
davr trace --json
```

### `davr snapshot`
Creates and manages Git-based repository snapshots without modifying git branch pointers.
```bash
davr snapshot create --reason "Before big refactoring"
davr snapshot list
davr snapshot show <SNAPSHOT_ID>
```

### `davr rollback`
Safely reverts working tree modifications made during an agent session ($A \cap B$) with transactional journal protection.
```bash
davr rollback <SNAPSHOT_OR_SESSION_ID> --dry-run
davr rollback <SNAPSHOT_OR_SESSION_ID> --yes
davr rollback <SNAPSHOT_OR_SESSION_ID> --force # Overwrites developer conflicts
```

### `davr diff`
Inspects changes between a snapshot and the current working tree.
```bash
davr diff <SNAPSHOT_ID>
```

### `davr test`
Runs test discovery and execution across detected frameworks (Cargo, Pytest, Jest, Go Test).
```bash
davr test
davr test --framework cargo_test --filter test_add
```

### `davr analyze`
Indexes project source files using Tree-sitter, extracting structured symbols and cross-file dependency edges into SQLite.
```bash
davr analyze
```

### `davr impact`
Computes transitive dependency graphs from modified files to identify affected source components and tests.
```bash
davr impact
davr impact --max-depth 4
```

### `davr flaky`
Executes stress testing over repeat iterations to identify nondeterministic tests.
```bash
davr flaky --iterations 10
```

### `davr db`
Database administration, backup, and health checks.
```bash
davr db stats
davr db verify
davr db backup --out backup.db
davr db migrate
```

### `davr clean`
Prunes expired snapshots, telemetry events, and verification records according to retention rules.
```bash
davr clean --older-than 30
davr clean --all
```

### `davr export`
Extracts telemetry events to newline-delimited JSON (JSONL) or JSON format.
```bash
davr export --format jsonl
davr export --session <SESSION_ID> --out trace.json
```

### `davr config`
Reads, validates, and updates configuration settings.
```bash
davr config show
davr config validate
davr config get agent.default_agent
davr config set agent.timeout_seconds 120
```

### `davr mcp`
Starts a Model Context Protocol (MCP) JSON-RPC stdio server exposing DAVR verification tools to AI IDEs.
```bash
davr mcp
```

### `davr version`
Prints version and build metadata.
```bash
davr version
```
