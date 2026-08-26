# DAVR Architecture & Design

This document details the architectural design, internal subsystems, and data flows of the Deterministic Agent Verification Runtime (DAVR).

---

## 1. Design Philosophy

> **"AI generates. DAVR verifies."**

Autonomous AI coding agents operating directly on a developer workstation introduce non-deterministic risks:
- Clashing with concurrent uncommitted work.
- Executing destructive filesystem or shell operations.
- Leaving orphan processes and modified repositories in an indeterminate state.

DAVR operates as a **local safety supervisor and deterministic verification runtime**. It wraps agent execution without requiring cloud connectivity, heavy virtualization, or container overhead.

---

## 2. High-Level Subsystem Map

DAVR is organized into 16 crates with strict dependency direction:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                 davr-cli                                    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼──────────────────────────────────────┐
│                                 davr-core                                   │
└──────────────┬───────────────────────┬───────────────────────┬──────────────┘
               │                       │                       │
 ┌─────────────▼────────────┐ ┌────────▼────────────┐ ┌────────▼────────────┐
 │        davr-env          │ │    davr-security    │ │      davr-git       │
 └─────────────┬────────────┘ └────────┬────────────┘ └────────┬────────────┘
               │                       │                       │
 ┌─────────────▼────────────┐ ┌────────▼────────────┐ ┌────────▼────────────┐
 │        davr-agent        │ │      davr-fs        │ │   davr-telemetry    │
 └─────────────┬────────────┘ └────────┬────────────┘ └────────┬────────────┘
               │                       │                       │
 ┌─────────────▼────────────┐ ┌────────▼────────────┐ ┌────────▼────────────┐
 │        davr-ast          │ │    davr-impact      │ │      davr-test      │
 └─────────────┬────────────┘ └────────┬────────────┘ └────────┬────────────┘
               │                       │                       │
 ┌─────────────▼────────────┐ ┌────────▼────────────┐ ┌────────▼────────────┐
 │        davr-flaky        │ │      davr-mcp       │ │    davr-config      │
 └─────────────┬────────────┘ └────────┬────────────┘ └────────┬────────────┘
               │                       │                       │
 ┌─────────────▼───────────────────────▼───────────────────────▼────────────┐
 │                               davr-storage                                 │
 └─────────────────────────────────────┬──────────────────────────────────────┘
                                       │
 ┌─────────────────────────────────────▼──────────────────────────────────────┐
 │                                davr-types                                  │
 └────────────────────────────────────────────────────────────────────────────┘
```

### Dependency Rules
1. `davr-types` contains domain IDs (`ProjectId`, `SessionId`, `SnapshotId`, `RollbackId`), the `FileState` enum, and the central `DavrError` taxonomy. It has zero internal crate dependencies.
2. `davr-storage` provides SQLite persistence in WAL mode. Domain crates interact with `Database` through typed transactions.
3. Domain crates (`davr-env`, `davr-security`, `davr-git`, `davr-fs`, `davr-agent`, etc.) encapsulate single-responsibility subsystems.
4. `davr-core` acts as the orchestrator wiring domain crates together.
5. `davr-cli` handles argument parsing, ANSI formatting, and JSON output serialization.

---

## 3. The Supervised Execution Lifecycle (`davr run`)

When a developer executes `davr run -- <command> [args...]`, the following deterministic pipeline executes:

```
[1] Pre-Flight Doctor Check
       │
       ▼
[2] Top-Level Security Policy Evaluation & Secret Redaction
       │ (Blocked commands terminate immediately with exit code 20)
       ▼
[3] Git ODB Tree Snapshot
       │ (Writes git tree object; records ref in refs/davr/snapshots/<id>)
       ▼
[4] Start Filesystem Event Monitor
       │ (Spawns notify watcher with 300ms debounce buffer)
       ▼
[5] Spawn Supervised Process Group
       │ (Spawns agent process under setpgid; applies environment allowlist)
       ▼
[6] Drain Filesystem Events + Git Diff Fail-Safe Reconciliation
       │ (Ensures rapid or bulk mutations missed by watcher are captured)
       ▼
[7] Capture Post-Session Hash States (B)
       │ (Computes BLAKE3 hashes for all touched files; persists in SQLite)
       ▼
[8] Telemetry Flush & Session Finalization
```

---

## 4. Rollback Architecture & Conflict Invariants

### 3-Way State Conflict Resolution ($A \to B \to C$)
When `davr rollback` is requested, `RollbackPlanner` computes operations using three distinct points in time:
- **$A$ (Snapshot State):** Content from the base Git tree snapshot.
- **$B$ (Post-Agent State):** Content hash captured at the exact moment the agent session finalized.
- **$C$ (Current State):** Content hash of the file currently present on disk.

```
                  ┌──────────────────────┐
                  │ Pre-Session Snap (A) │
                  └──────────┬───────────┘
                             │  (Agent Mutates)
                             ▼
                  ┌──────────────────────┐
                  │  Post-Agent State (B)│
                  └──────────┬───────────┘
                             │  (Developer Edits or Leaves As Is)
                             ▼
                  ┌──────────────────────┐
                  │   Current State (C)  │
                  └──────────┬───────────┘
                             │
            ┌────────────────┴────────────────┐
            │                                 │
     If C == B                         If C != B
            │                                 │
            ▼                                 ▼
   [Safe Rollback Path]              [Conflict Detected]
   Restores A or Deletes             Preserves C on disk;
   Agent-created file                Requires --force to overwrite
```

### Transactional Rollback Journaling
File mutations during rollback are managed by `RollbackExecutor`:
1. **Prepare:** Generates unique `RollbackId` and creates `.davr/rollback-txn/<id>/`.
2. **Backup:** Copies every target file to `.davr/rollback-txn/<id>/backups/`.
3. **Apply:** Restores blob contents from Git ODB or removes agent-created files.
4. **Commit / Abort:** If an error occurs at any point during step 3, all staged backups are copied back to their original locations, guaranteeing no partial or corrupt state remains. On success, the transaction journal is pruned.

---

## 5. Storage Engine & Database Schema

DAVR embeds SQLite using `rusqlite` with the following configuration:
- `PRAGMA journal_mode = WAL;` (Concurrent readers, non-blocking writes)
- `PRAGMA foreign_keys = ON;`
- `PRAGMA busy_timeout = 5000;`

### Primary Schema Tables
- `projects`: Root project registrations and metadata.
- `agent_sessions`: Executed agent runs, sanitized command lines, durations, and exit codes.
- `git_snapshots`: Tree hashes, snapshot triggers, and dirty state flags.
- `filesystem_events`: Real-time file creation, modification, and deletion event records.
- `post_session_file_states`: File path to BLAKE3 hash mappings recorded post-execution.
- `rollback_operations`: Audit logs for all completed or aborted rollbacks.
- `telemetry_events`: Structured event traces (`SESSION_STARTED`, `SESSION_FINISHED`, `SNAPSHOT_CREATED`, `COMMAND_BLOCKED`).
- `test_runs` & `flaky_test_runs`: Test suite execution results and flakiness stability classifications.
- `source_files`, `source_symbols`, `dependency_edges`: Source AST index and dependency graph.
