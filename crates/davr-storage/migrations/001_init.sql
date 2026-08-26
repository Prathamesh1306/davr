-- =====================================================================
-- DAVR SQLite Schema — davr-storage crate
-- Applied via sequential numbered migrations; 001_init.sql
-- =====================================================================

PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------
-- Migration tracking
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY,          -- sequential, e.g. 1, 2, 3...
    description TEXT NOT NULL,
    applied_at  INTEGER NOT NULL              -- unix ms
);

-- ---------------------------------------------------------------------
-- Group A: Project & Agent Execution Hierarchy
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS projects (
    id                 TEXT PRIMARY KEY,       -- uuidv4
    name               TEXT NOT NULL,
    root_path          TEXT NOT NULL UNIQUE,   -- absolute path, one row per .davr/ root
    default_language   TEXT,                   -- nullable; inferred, not required
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS repositories (
    id            TEXT PRIMARY KEY,             -- uuidv4
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,                 -- path from project root; '' for root repo
    remote_url    TEXT,                          -- nullable
    default_branch TEXT,
    created_at    INTEGER NOT NULL,
    UNIQUE(project_id, relative_path)
);

CREATE TABLE IF NOT EXISTS agent_sessions (
    id               TEXT PRIMARY KEY,          -- uuidv4
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    agent_name       TEXT NOT NULL,              -- 'claude' | 'aider' | 'opencode' | 'generic'
    command_line     TEXT NOT NULL,              -- redacted argv, secrets stripped
    status           TEXT NOT NULL CHECK (status IN ('running','completed','failed','aborted')),
    pre_snapshot_id  TEXT REFERENCES git_snapshots(id) ON DELETE SET NULL,
    post_snapshot_id TEXT REFERENCES git_snapshots(id) ON DELETE SET NULL,
    started_at       INTEGER NOT NULL,
    finished_at      INTEGER,                    -- NULL while running
    exit_code        INTEGER
);

CREATE TABLE IF NOT EXISTS agent_runs (
    id             TEXT PRIMARY KEY,             -- uuidv4
    session_id     TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    parent_run_id  TEXT REFERENCES agent_runs(id) ON DELETE CASCADE, -- NULL = top-level run
    label          TEXT,                          -- adapter-supplied subagent label, nullable
    status         TEXT NOT NULL CHECK (status IN ('running','completed','failed','aborted')),
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER
);

CREATE TABLE IF NOT EXISTS agent_iterations (
    id            INTEGER PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    iteration_index INTEGER NOT NULL,             -- 0-based, monotonic within run
    summary_hash  TEXT,                            -- fingerprint of observable state, for loop detection
    loop_flag     INTEGER NOT NULL DEFAULT 0 CHECK (loop_flag IN (0,1)),
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    UNIQUE(run_id, iteration_index)
);

CREATE TABLE IF NOT EXISTS commands (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    run_id          TEXT REFERENCES agent_runs(id) ON DELETE CASCADE,
    iteration_id    INTEGER REFERENCES agent_iterations(id) ON DELETE SET NULL,
    raw_command     TEXT NOT NULL,                 -- secret-redacted, via davr-security
    policy_decision TEXT NOT NULL CHECK (policy_decision IN ('allowed','blocked','confirmed_by_user')),
    blocked_reason  TEXT,
    exit_code       INTEGER,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER
);

CREATE TABLE IF NOT EXISTS processes (
    id                 INTEGER PRIMARY KEY,
    command_id         INTEGER NOT NULL REFERENCES commands(id) ON DELETE CASCADE,
    parent_process_id  INTEGER REFERENCES processes(id) ON DELETE CASCADE,
    os_pid             INTEGER,                    -- nullable: may be unavailable on some platforms mid-failure
    exit_code          INTEGER,
    signal             TEXT,                        -- e.g. 'SIGTERM', nullable
    started_at         INTEGER NOT NULL,
    finished_at        INTEGER
);

-- ---------------------------------------------------------------------
-- Group B: Filesystem & Git Safety
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS git_snapshots (
    id           TEXT PRIMARY KEY,                 -- uuidv4
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id   TEXT REFERENCES agent_sessions(id) ON DELETE SET NULL,
    tree_hash    TEXT NOT NULL,                     -- libgit2 tree OID
    reason       TEXT NOT NULL CHECK (reason IN ('pre_run','pre_mutation','manual','pre_rollback')),
    dirty_before INTEGER NOT NULL CHECK (dirty_before IN (0,1)),
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS rollback_operations (
    id                  TEXT PRIMARY KEY,           -- uuidv4
    project_id          TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id         TEXT NOT NULL REFERENCES git_snapshots(id) ON DELETE RESTRICT,
    session_id          TEXT REFERENCES agent_sessions(id) ON DELETE SET NULL,
    status              TEXT NOT NULL CHECK (status IN ('succeeded','failed','partial')),
    files_restored_count INTEGER,
    error_message       TEXT,
    initiated_at        INTEGER NOT NULL,
    completed_at        INTEGER
);

CREATE TABLE IF NOT EXISTS filesystem_events (
    id                INTEGER PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    file_path         TEXT NOT NULL,
    old_path          TEXT,                          -- populated for 'renamed' only
    event_type        TEXT NOT NULL CHECK (event_type IN ('created','modified','deleted','renamed')),
    confidence        TEXT NOT NULL DEFAULT 'high' CHECK (confidence IN ('high','low')),
    content_hash_after TEXT,                          -- blake3, nullable for 'deleted'
    detected_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS file_versions (
    id           INTEGER PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id  TEXT REFERENCES git_snapshots(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    blob_hash    TEXT NOT NULL,                       -- libgit2 blob OID
    size_bytes   INTEGER NOT NULL,
    recorded_at  INTEGER NOT NULL
);

-- ---------------------------------------------------------------------
-- Group C: Environment
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS environment_checks (
    id           INTEGER PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id   TEXT REFERENCES agent_sessions(id) ON DELETE SET NULL,  -- NULL: standalone `davr doctor`
    check_name   TEXT NOT NULL,                        -- e.g. 'python_runtime_present'
    category     TEXT NOT NULL CHECK (category IN
                   ('os','path','runtime','package_manager','lockfile','git','docker',
                    'env_var','credential','permission','repo_state')),
    status       TEXT NOT NULL CHECK (status IN ('pass','fail','warn','skipped')),
    detail       TEXT,                                  -- JSON, human-readable context
    checked_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS installed_tools (
    id                    INTEGER PRIMARY KEY,
    project_id            TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    environment_check_id  INTEGER REFERENCES environment_checks(id) ON DELETE CASCADE,
    tool_name             TEXT NOT NULL,
    version                TEXT,
    resolved_path          TEXT,
    detected_at            INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS dependencies (
    id               INTEGER PRIMARY KEY,
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    ecosystem        TEXT NOT NULL CHECK (ecosystem IN ('npm','pip','cargo','go')),
    name             TEXT NOT NULL,
    declared_version TEXT,
    resolved_version TEXT,
    manifest_path    TEXT NOT NULL,
    detected_at      INTEGER NOT NULL
);

-- ---------------------------------------------------------------------
-- Group D: Telemetry
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS telemetry_events (
    id          INTEGER PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id  TEXT REFERENCES agent_sessions(id) ON DELETE SET NULL,
    kind        TEXT NOT NULL,                          -- e.g. SESSION_STARTED, COMMAND_STARTED
    severity    TEXT NOT NULL CHECK (severity IN ('debug','info','warn','error')),
    ref_table   TEXT,                                    -- e.g. 'commands', nullable
    ref_id      TEXT,                                    -- id in ref_table, nullable
    payload     TEXT,                                     -- JSON, event-specific fields
    occurred_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS token_usage (
    id           INTEGER PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    run_id       TEXT REFERENCES agent_runs(id) ON DELETE CASCADE,
    iteration_id INTEGER REFERENCES agent_iterations(id) ON DELETE CASCADE,
    available    INTEGER NOT NULL CHECK (available IN (0,1)), -- 0 = agent did not expose metrics
    input_tokens  INTEGER,
    output_tokens INTEGER,
    cached_tokens INTEGER,
    total_tokens  INTEGER,
    cost_usd      REAL,
    recorded_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS context_metrics (
    id                 INTEGER PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    iteration_id       INTEGER REFERENCES agent_iterations(id) ON DELETE CASCADE,
    context_fill_ratio REAL,                              -- nullable if agent doesn't expose it
    warning_triggered  INTEGER NOT NULL DEFAULT 0 CHECK (warning_triggered IN (0,1)),
    recorded_at        INTEGER NOT NULL
);

-- ---------------------------------------------------------------------
-- Group E: AST / Impact Analysis
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS source_files (
    id            TEXT PRIMARY KEY,                       -- uuidv4
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    file_path     TEXT NOT NULL,
    language      TEXT NOT NULL CHECK (language IN ('typescript','javascript','python','rust','go')),
    content_hash  TEXT NOT NULL,                           -- blake3, drives re-parse decisions
    last_parsed_at INTEGER NOT NULL,
    UNIQUE(project_id, file_path)
);

CREATE TABLE IF NOT EXISTS source_symbols (
    id             TEXT PRIMARY KEY,                       -- uuidv4
    source_file_id TEXT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    symbol_name    TEXT NOT NULL,
    symbol_kind    TEXT NOT NULL CHECK (symbol_kind IN
                    ('function','method','class','interface','struct','enum','const','import')),
    start_byte     INTEGER NOT NULL,
    end_byte       INTEGER NOT NULL,
    start_line     INTEGER NOT NULL,
    end_line       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS dependency_edges (
    id             INTEGER PRIMARY KEY,
    from_file_id   TEXT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    to_file_id     TEXT REFERENCES source_files(id) ON DELETE CASCADE,     -- nullable: external/unresolved import
    from_symbol_id TEXT REFERENCES source_symbols(id) ON DELETE CASCADE,
    to_symbol_id   TEXT REFERENCES source_symbols(id) ON DELETE CASCADE,
    edge_kind      TEXT NOT NULL CHECK (edge_kind IN ('import','call','reference','extends','implements')),
    confidence     TEXT NOT NULL CHECK (confidence IN ('high','medium','low'))
);

-- ---------------------------------------------------------------------
-- Group F: Testing & Verification
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS test_files (
    id           TEXT PRIMARY KEY,                         -- uuidv4
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    framework    TEXT NOT NULL CHECK (framework IN ('pytest','jest','vitest','cargo_test','go_test')),
    UNIQUE(project_id, file_path)
);

CREATE TABLE IF NOT EXISTS test_cases (
    id           TEXT PRIMARY KEY,                         -- uuidv4
    test_file_id TEXT NOT NULL REFERENCES test_files(id) ON DELETE CASCADE,
    test_name    TEXT NOT NULL,
    UNIQUE(test_file_id, test_name)
);

CREATE TABLE IF NOT EXISTS verification_runs (
    id          TEXT PRIMARY KEY,                          -- uuidv4
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id  TEXT REFERENCES agent_sessions(id) ON DELETE SET NULL,  -- NULL: CI-triggered, no session
    trigger     TEXT NOT NULL CHECK (trigger IN ('manual','agent_session','ci')),
    status      TEXT NOT NULL CHECK (status IN ('running','passed','failed','error')),
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE TABLE IF NOT EXISTS impacted_files (
    id                  INTEGER PRIMARY KEY,
    verification_run_id TEXT NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    source_file_id      TEXT NOT NULL REFERENCES source_files(id) ON DELETE CASCADE,
    confidence          TEXT NOT NULL CHECK (confidence IN ('high','medium','low')),
    reason              TEXT
);

CREATE TABLE IF NOT EXISTS impacted_tests (
    id                  INTEGER PRIMARY KEY,
    verification_run_id TEXT NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    test_case_id        TEXT NOT NULL REFERENCES test_cases(id) ON DELETE CASCADE,
    triggered_by_file_id TEXT REFERENCES source_files(id) ON DELETE SET NULL,
    confidence          TEXT NOT NULL CHECK (confidence IN ('high','medium','low'))
);

CREATE TABLE IF NOT EXISTS test_runs (
    id                   TEXT PRIMARY KEY,                 -- uuidv4
    verification_run_id  TEXT NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    framework            TEXT NOT NULL,
    iteration_index      INTEGER NOT NULL DEFAULT 0,        -- 0 = normal; >0 = flaky stress iteration N
    exit_code            INTEGER,
    started_at           INTEGER NOT NULL,
    finished_at          INTEGER
);

CREATE TABLE IF NOT EXISTS test_results (
    id           INTEGER PRIMARY KEY,
    test_run_id  TEXT NOT NULL REFERENCES test_runs(id) ON DELETE CASCADE,
    test_case_id TEXT NOT NULL REFERENCES test_cases(id) ON DELETE CASCADE,
    status       TEXT NOT NULL CHECK (status IN ('passed','failed','skipped','timeout','error')),
    duration_ms  INTEGER,
    error_message TEXT
);

CREATE TABLE IF NOT EXISTS flaky_test_runs (
    id                   TEXT PRIMARY KEY,                 -- uuidv4
    test_case_id         TEXT NOT NULL REFERENCES test_cases(id) ON DELETE CASCADE,
    verification_run_id  TEXT NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    iterations_run        INTEGER NOT NULL,
    pass_count             INTEGER NOT NULL,
    fail_count              INTEGER NOT NULL,
    timeout_count            INTEGER NOT NULL,
    classification            TEXT NOT NULL CHECK (classification IN
                               ('STABLE_PASS','STABLE_FAIL','FLAKY','TIMEOUT_UNSTABLE','INFRA_FAILURE','UNKNOWN')),
    classified_at              INTEGER NOT NULL
);

-- ---------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_agent_runs_session          ON agent_runs(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_iterations_run        ON agent_iterations(run_id);
CREATE INDEX IF NOT EXISTS idx_commands_session            ON commands(session_id);
CREATE INDEX IF NOT EXISTS idx_commands_iteration          ON commands(iteration_id);
CREATE INDEX IF NOT EXISTS idx_processes_command           ON processes(command_id);
CREATE INDEX IF NOT EXISTS idx_filesystem_events_session   ON filesystem_events(session_id);
CREATE INDEX IF NOT EXISTS idx_git_snapshots_session       ON git_snapshots(session_id);
CREATE INDEX IF NOT EXISTS idx_environment_checks_session  ON environment_checks(session_id);
CREATE INDEX IF NOT EXISTS idx_token_usage_session         ON token_usage(session_id);
CREATE INDEX IF NOT EXISTS idx_context_metrics_session     ON context_metrics(session_id);

CREATE INDEX IF NOT EXISTS idx_telemetry_events_time       ON telemetry_events(occurred_at);
CREATE INDEX IF NOT EXISTS idx_filesystem_events_time      ON filesystem_events(detected_at);
CREATE INDEX IF NOT EXISTS idx_commands_time               ON commands(started_at);

CREATE INDEX IF NOT EXISTS idx_telemetry_events_kind       ON telemetry_events(kind);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_status       ON agent_sessions(status);
CREATE INDEX IF NOT EXISTS idx_verification_runs_status    ON verification_runs(status);
CREATE INDEX IF NOT EXISTS idx_verification_runs_project   ON verification_runs(project_id);

CREATE INDEX IF NOT EXISTS idx_impacted_files_run          ON impacted_files(verification_run_id);
CREATE INDEX IF NOT EXISTS idx_impacted_tests_run          ON impacted_tests(verification_run_id);
CREATE INDEX IF NOT EXISTS idx_test_results_run            ON test_results(test_run_id);
CREATE INDEX IF NOT EXISTS idx_dependency_edges_from       ON dependency_edges(from_file_id);
CREATE INDEX IF NOT EXISTS idx_dependency_edges_to         ON dependency_edges(to_file_id);
CREATE INDEX IF NOT EXISTS idx_flaky_test_runs_case        ON flaky_test_runs(test_case_id);
